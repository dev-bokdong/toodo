//! Commands for tasks, their dependency DAG, and their commit records.

use super::State;

use super::types::{
    self, CommitRecord, CriterionId, NewTask, Task, TaskEdit, TaskId, TaskRelation,
};
use super::{CommandError, CommandResult, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::domain::criteria::{self, CheckKind, CheckedBy, NewCriterion};
use crate::domain::model;
use crate::domain::model::TaskResolution;
use crate::domain::status::TaskStatus;
use crate::storage::{self, CriterionEdit, Storage, StorageError};

/// Tasks of every workspace ordered by ID, each with its dependencies, which may belong
/// to another workspace.
#[specta::specta]
pub fn list_tasks(storage: State<'_, StorageState>) -> CommandResult<Vec<Task>> {
    Ok(types::map_all(&lock(&storage).list_all_tasks()?))
}

#[specta::specta]
pub fn get_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    let task = lock(&storage)
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

/// Creates a `planned` task without dependencies, with its completion criteria. The merge
/// criterion depends on whether the repository has an `origin` remote, which git may
/// check over `ssh`, so this runs off the main thread.
#[specta::specta]
pub fn create_task(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    task: NewTask,
) -> CommandResult<Task> {
    Ok((&create(&storage, &git, task)?).into())
}

/// Creates a task as [`create_task`] does.
pub(crate) fn create(
    storage: &StorageState,
    git: &Git,
    task: NewTask,
) -> CommandResult<model::Task> {
    let wants_merge = task.merge_criterion;
    let mut new: storage::NewTask = task.try_into()?;
    if wants_merge
        && let Some(merge) = merge_criterion(storage, git, new.workspace_id, &new.criteria)?
    {
        new.criteria.insert(0, merge);
    }
    Ok(lock(storage).create_task(&new)?)
}

/// The merge criterion a new task of `workspace` gets in front of `criteria`, unless they
/// already have one: its pull request is merged, or, in a repository without an `origin`
/// remote, its branch is merged locally. Git runs without the database lock. A local
/// repository the server cannot reach (see [`types::PathProblem`]) is not asked and gets
/// the pull request criterion, so its tasks can still be planned.
pub(crate) fn merge_criterion(
    storage: &StorageState,
    git: &Git,
    workspace: model::WorkspaceId,
    criteria: &[NewCriterion],
) -> CommandResult<Option<NewCriterion>> {
    let has_merge = criteria.iter().any(|criterion| {
        matches!(
            criterion.kind,
            CheckKind::PullRequestMerged | CheckKind::BranchMergedLocally
        )
    });
    if has_merge {
        return Ok(None);
    }
    let workspace = lock(storage)
        .get_workspace(workspace)?
        .ok_or(StorageError::WorkspaceNotFound(workspace))?;
    let has_origin = match types::PathProblem::of(&workspace.location) {
        Some(_) => true,
        None => git.has_origin(&workspace.location)?,
    };
    Ok(Some(NewCriterion::merge(has_origin)))
}

// The user edits the criteria list row by row in the app, so removing the merge
// criterion there is deliberate.
#[specta::specta]
pub fn update_task(storage: State<'_, StorageState>, task: TaskEdit) -> CommandResult<Task> {
    edit_task(&mut lock(&storage), task, true)
}

/// Edits a task. New `criteria` replace the current ones; unless `may_remove_merge`, a
/// list that would delete the task's merge criterion is rejected.
pub(crate) fn edit_task(
    storage: &mut Storage,
    edit: TaskEdit,
    may_remove_merge: bool,
) -> CommandResult<Task> {
    let title = edit.title.trim();
    if title.is_empty() {
        return Err(CommandError::invalid_input("task title must not be empty"));
    }
    let mut task = storage
        .get_task(edit.id.into())?
        .ok_or(StorageError::TaskNotFound(edit.id.into()))?;
    task.task_type = edit.task_type.into();
    task.title = title.to_owned();
    task.description = edit.description;
    task.estimate = types::estimate(edit.estimate)?;
    task.due_date = types::due_date(edit.due_date.as_deref())?;
    if let Some(criteria) = &edit.criteria {
        if task.status == TaskStatus::Done {
            return Err(StorageError::CriteriaOfDoneTask(task.id).into());
        }
        let edits = criterion_edits(storage, &task, criteria, may_remove_merge)?;
        storage.set_criteria(task.id, &edits)?;
    }
    storage.update_task(&task)?;
    let task = storage
        .get_task(task.id)?
        .ok_or(StorageError::TaskNotFound(task.id))?;
    Ok((&task).into())
}

fn is_merge(kind: CheckKind) -> bool {
    matches!(
        kind,
        CheckKind::PullRequestMerged | CheckKind::BranchMergedLocally
    )
}

/// The criteria `inputs` ask `task` to have, in order. An input with an `id` is an
/// existing criterion of the task whose omitted fields keep their stored values; each
/// one appears once.
fn criterion_edits(
    storage: &Storage,
    task: &model::Task,
    inputs: &[types::CriterionInput],
    may_remove_merge: bool,
) -> CommandResult<Vec<CriterionEdit>> {
    let mut edits: Vec<CriterionEdit> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let Some(id) = input.id else {
            edits.push(CriterionEdit {
                id: None,
                criterion: input.parse()?,
            });
            continue;
        };
        let id: criteria::CriterionId = id.into();
        if edits.iter().any(|edit| edit.id == Some(id)) {
            return Err(CommandError::invalid_input(format!(
                "criterion {id} appears twice; send each criterion once"
            )));
        }
        let Some(known) = task.criteria.iter().find(|known| known.id == id) else {
            let owner = match storage.criterion_task(id)? {
                Some(owner) => format!(
                    "criterion {id} belongs to task {owner}, not task {}",
                    task.id
                ),
                None => format!("criterion {id} does not exist"),
            };
            return Err(CommandError::invalid_input(format!(
                "{owner}; call get_task {} for the ids of its criteria, or leave out id to add a new criterion",
                task.id
            )));
        };
        edits.push(CriterionEdit {
            id: Some(id),
            criterion: input.parse_over(known)?,
        });
    }
    if !may_remove_merge
        && let Some(merge) = task.criteria.iter().find(|known| is_merge(known.kind))
        && !edits.iter().any(|edit| is_merge(edit.criterion.kind))
    {
        return Err(CommandError::rejected(format!(
            "these criteria would delete the merge criterion {} ({}) of task {} with its evidence; keep it by adding {{\"id\": {}}} to criteria, or pass remove_merge_criterion: true to remove it deliberately",
            merge.id, merge.kind, task.id, merge.id
        )));
    }
    Ok(edits)
}

/// Checks (`met`) or unchecks a manual completion criterion of a task that is not done,
/// with what shows it; blank evidence is none. A task in progress whose criteria are then
/// all met awaits completion until it is completed.
#[specta::specta]
pub fn check_criterion(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    criterion_id: CriterionId,
    met: bool,
    evidence: Option<String>,
) -> CommandResult<Task> {
    check(
        &mut lock(&storage),
        Some(task_id),
        criterion_id,
        met,
        evidence.as_deref(),
        CheckedBy::User,
    )
}

/// Checks a manual criterion as [`check_criterion`] does, as `by`. Without `task_id`,
/// the task is the one the criterion belongs to; with one, it must be that task.
pub(crate) fn check(
    storage: &mut Storage,
    task_id: Option<TaskId>,
    criterion_id: CriterionId,
    met: bool,
    evidence: Option<&str>,
    by: CheckedBy,
) -> CommandResult<Task> {
    let evidence = evidence
        .map(str::trim)
        .filter(|evidence| !evidence.is_empty());
    let criterion: criteria::CriterionId = criterion_id.into();
    let Some(owner) = storage.criterion_task(criterion)? else {
        return Err(CommandError::not_found(format!(
            "criterion {criterion} does not exist; call get_task on your task for the ids of its criteria"
        )));
    };
    if let Some(task_id) = task_id
        && model::TaskId::from(task_id) != owner
    {
        return Err(CommandError::invalid_input(format!(
            "criterion {criterion} belongs to task {owner}, not task {}; call check_criterion with task_id {owner}, or without task_id",
            task_id.0
        )));
    }
    let task = storage.check_criterion(owner, criterion, met, evidence, by)?;
    Ok((&task).into())
}

/// Pauses an `in_progress` or `in_review` task: it becomes `planned` again and keeps
/// its branch, worktree, herdr workspace, commit records, and pull request, so starting
/// it again resumes the work.
#[specta::specta]
pub fn pause_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    pause(&mut lock(&storage), id)
}

pub(crate) fn pause(storage: &mut Storage, id: TaskId) -> CommandResult<Task> {
    let task = storage
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    if task.status == TaskStatus::Planned {
        return Err(CommandError::rejected(format!(
            "task {} is not being worked on",
            task.id
        )));
    }
    Ok((&storage.set_task_status(id.into(), TaskStatus::Planned)?).into())
}

/// Stores what the user wrote about the outcome of a `done` task, for reviews. The note
/// is trimmed; an empty one clears it, except for a task that was not adopted, whose note
/// is its required conclusion.
#[specta::specta]
pub fn set_completion_note(
    storage: State<'_, StorageState>,
    id: TaskId,
    note: String,
) -> CommandResult<Task> {
    note_completion(&mut lock(&storage), id, &note)
}

fn note_completion(storage: &mut Storage, id: TaskId, note: &str) -> CommandResult<Task> {
    let task = storage
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    if task.status != TaskStatus::Done {
        return Err(CommandError::rejected(format!(
            "task {} is {}; only a done task has a completion note",
            task.id, task.status
        )));
    }
    let note = Some(note.trim()).filter(|note| !note.is_empty());
    if note.is_none() && task.resolution == Some(TaskResolution::NotAdopted) {
        return Err(CommandError::invalid_input(
            "the conclusion of work that was not adopted cannot be cleared",
        ));
    }
    Ok((&storage.set_completion_note(id.into(), note)?).into())
}

/// Deletes a task with its dependency edges and commit records. A task that still has a worktree
/// first removes it through herdr, which also closes its herdr workspace; the branch is
/// kept. Without `force`, a worktree with modified or untracked files is rejected and the
/// task is kept.
#[specta::specta]
pub fn delete_task(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
    force: bool,
) -> CommandResult<()> {
    super::herdr::delete(&storage, &herdr, id.into(), force)
}

/// Makes `task_id` depend on `depends_on`, which may belong to another workspace. Rejects
/// self and cyclic edges. An `in_progress` task that the new prerequisite blocks is
/// paused and its herdr workspace closed.
#[specta::specta]
pub fn add_dependency(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    super::herdr::add_dependencies(&storage, &herdr, task_id.into(), &[depends_on.into()])?;
    Ok(())
}

#[specta::specta]
pub fn remove_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).remove_dependency(task_id.into(), depends_on.into())?)
}

/// Every related link between tasks, which neither block nor wait on each other.
#[specta::specta]
pub fn list_task_relations(storage: State<'_, StorageState>) -> CommandResult<Vec<TaskRelation>> {
    Ok(lock(&storage)
        .list_relations()?
        .into_iter()
        .map(|(task, related)| TaskRelation {
            task_id: task.into(),
            related_id: related.into(),
        })
        .collect())
}

/// Links two tasks, from any workspaces, as related; linking twice keeps one link.
#[specta::specta]
pub fn add_task_relation(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    related_id: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).add_related(task_id.into(), related_id.into())?)
}

/// Removes the related link between two tasks, given in either order.
#[specta::specta]
pub fn remove_task_relation(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    related_id: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).remove_related(task_id.into(), related_id.into())?)
}

#[specta::specta]
pub fn list_commit_records(
    storage: State<'_, StorageState>,
    task_id: TaskId,
) -> CommandResult<Vec<CommitRecord>> {
    Ok(types::map_all(
        &lock(&storage).list_commit_records(task_id.into())?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;
    use crate::domain::model::{self, WorkspaceLocation};
    use std::sync::Arc;

    use crate::adapters::git::fake::FakeRunner;
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::storage::{NewTask, NewWorkspace};

    fn storage_with_task() -> (Storage, model::Task) {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "toodo".to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let mut task = storage
            .create_task(&NewTask {
                workspace_id: workspace.id,
                task_type: model::TaskType::Feat,
                title: "Add commands".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
                criteria: Vec::new(),
            })
            .unwrap();
        task.branch = Some("feat/add-commands-1".to_owned());
        storage.update_task(&task).unwrap();
        (storage, task)
    }

    fn new_task(workspace_id: model::WorkspaceId) -> types::NewTask {
        types::NewTask {
            workspace_id: workspace_id.into(),
            task_type: types::TaskType::Feat,
            title: "Add login".to_owned(),
            description: String::new(),
            estimate: None,
            due_date: None,
            criteria: Vec::new(),
            merge_criterion: true,
        }
    }

    fn git(results: Vec<Result<String, crate::adapters::git::GitError>>) -> (Git, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (Git::new(Box::new(runner.clone())), runner)
    }

    fn storage_with_workspace(location: WorkspaceLocation) -> (StorageState, model::WorkspaceId) {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "quest".to_owned(),
                location,
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        (StorageState::new(storage), workspace.id)
    }

    #[test]
    fn a_task_of_a_workspace_the_server_cannot_reach_is_created_without_git() {
        let missing = tempfile::tempdir().unwrap().path().join("gone");
        for location in [
            WorkspaceLocation::Local {
                path: WindowsPath::parse(r"D:\dev\quest").unwrap().into(),
            },
            WorkspaceLocation::Local {
                path: PosixPath::parse(missing.to_str().unwrap()).unwrap().into(),
            },
        ] {
            let (storage, workspace) = storage_with_workspace(location);
            let (git, runner) = git(Vec::new());
            let task = create(&storage, &git, new_task(workspace)).unwrap();
            assert_eq!(task.criteria[0].kind, CheckKind::PullRequestMerged);
            assert!(runner.invocations.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_task_of_a_reachable_repository_takes_its_merge_criterion_from_git() {
        let repo = tempfile::tempdir().unwrap();
        let (storage, workspace) = storage_with_workspace(WorkspaceLocation::Local {
            path: PosixPath::parse(repo.path().to_str().unwrap())
                .unwrap()
                .into(),
        });
        let (git, runner) = git(vec![Ok("upstream\n".to_owned())]);
        let task = create(&storage, &git, new_task(workspace)).unwrap();
        assert_eq!(task.criteria[0].kind, CheckKind::BranchMergedLocally);
        assert_eq!(runner.invocations.lock().unwrap().len(), 1);
    }

    #[test]
    fn edit_task_keeps_workflow_fields() {
        let (mut storage, task) = storage_with_task();
        let edited = edit_task(
            &mut storage,
            TaskEdit {
                id: task.id.into(),
                task_type: types::TaskType::Fix,
                title: "  Fix commands ".to_owned(),
                description: "details".to_owned(),
                estimate: Some(types::Estimate {
                    unit: types::EstimateUnit::Days,
                    amount: 3.0,
                }),
                due_date: Some(" 2026-10-31 ".to_owned()),
                criteria: None,
            },
            true,
        )
        .unwrap();
        assert_eq!(edited.title, "Fix commands");
        assert_eq!(edited.due_date.as_deref(), Some("2026-10-31"));
        assert_eq!(edited.estimate.map(|estimate| estimate.amount), Some(3.0));
        assert_eq!(edited.task_type, types::TaskType::Fix);
        assert_eq!(edited.branch.as_deref(), Some("feat/add-commands-1"));
        let stored = storage.get_task(task.id).unwrap().unwrap();
        assert_eq!(Task::from(&stored), edited);
    }

    #[test]
    fn pausing_moves_a_started_task_back_to_planned_and_keeps_its_branch() {
        let (mut storage, task) = storage_with_task();
        let error = pause(&mut storage, task.id.into()).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);

        for status in [TaskStatus::InProgress, TaskStatus::InReview] {
            storage
                .set_task_status(task.id, TaskStatus::InProgress)
                .unwrap();
            if status == TaskStatus::InReview {
                storage.set_task_status(task.id, status).unwrap();
            }
            let paused = pause(&mut storage, task.id.into()).unwrap();
            assert_eq!(paused.status, types::TaskStatus::Planned);
            assert_eq!(paused.branch.as_deref(), Some("feat/add-commands-1"));
        }
    }

    #[test]
    fn only_a_done_task_gets_a_trimmed_completion_note_and_an_empty_one_clears_it() {
        let (mut storage, task) = storage_with_task();
        let error = note_completion(&mut storage, task.id.into(), "Shipped").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        let error = note_completion(&mut storage, TaskId(999), "Shipped").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);

        for status in [
            TaskStatus::InProgress,
            TaskStatus::InReview,
            TaskStatus::Done,
        ] {
            storage.set_task_status(task.id, status).unwrap();
        }
        let noted =
            note_completion(&mut storage, task.id.into(), "  Cut login time in half\n").unwrap();
        assert_eq!(
            noted.completion_note.as_deref(),
            Some("Cut login time in half")
        );
        let stored = storage.get_task(task.id).unwrap().unwrap();
        assert_eq!(Task::from(&stored), noted);

        let cleared = note_completion(&mut storage, task.id.into(), " ").unwrap();
        assert_eq!(cleared.completion_note, None);
        assert_eq!(
            storage.get_task(task.id).unwrap().unwrap().completion_note,
            None
        );
    }

    #[test]
    fn the_conclusion_of_a_task_that_was_not_adopted_can_be_edited_but_not_cleared() {
        let (mut storage, task) = storage_with_task();
        storage
            .set_task_status(task.id, TaskStatus::InProgress)
            .unwrap();
        storage
            .conclude_task(task.id, "Too slow", &[], None)
            .unwrap();

        let error = note_completion(&mut storage, task.id.into(), " ").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let edited = note_completion(&mut storage, task.id.into(), "Twice as slow").unwrap();
        assert_eq!(edited.completion_note.as_deref(), Some("Twice as slow"));
        assert_eq!(edited.resolution, Some(types::TaskResolution::NotAdopted));
    }

    #[test]
    fn edit_task_rejects_empty_title_and_unknown_task() {
        let (mut storage, task) = storage_with_task();
        let edit = |id: TaskId, title: &str| TaskEdit {
            id,
            task_type: types::TaskType::Feat,
            title: title.to_owned(),
            description: String::new(),
            estimate: None,
            due_date: None,
            criteria: None,
        };
        let error = edit_task(&mut storage, edit(task.id.into(), " "), true).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = edit_task(&mut storage, edit(TaskId(999), "Other"), true).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(storage.get_task(task.id).unwrap().unwrap(), task);
    }
}
