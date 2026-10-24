//! Commands for tasks, their dependency DAG, and their commit records.

use super::State;

use super::types::{self, CommitRecord, CriterionId, NewTask, Task, TaskEdit, TaskId};
use super::{CommandError, CommandResult, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::domain::criteria::{CheckKind, CheckedBy, NewCriterion};
use crate::domain::model;
use crate::domain::model::TaskResolution;
use crate::domain::status::TaskStatus;
use crate::storage::{self, CriterionEdit, Storage, StorageError};

/// Tasks of every workspace ordered by ID, each with its dependencies, which may belong
/// to another workspace.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_tasks(storage: State<'_, StorageState>) -> CommandResult<Vec<Task>> {
    Ok(types::map_all(&lock(&storage).list_all_tasks()?))
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn get_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    let task = lock(&storage)
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

/// Creates a `planned` task without dependencies, with its completion criteria. The merge
/// criterion depends on whether the repository has an `origin` remote, which git may
/// check over `ssh`, so this runs off the main thread.
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
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
/// remote, its branch is merged locally. Git runs without the database lock.
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
    Ok(Some(NewCriterion::merge(
        git.has_origin(&workspace.location)?,
    )))
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn update_task(storage: State<'_, StorageState>, task: TaskEdit) -> CommandResult<Task> {
    edit_task(&mut lock(&storage), task)
}

pub(crate) fn edit_task(storage: &mut Storage, edit: TaskEdit) -> CommandResult<Task> {
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
        let edits = criteria
            .iter()
            .map(|input| {
                Ok(CriterionEdit {
                    id: input.id.map(Into::into),
                    criterion: input.parse()?,
                })
            })
            .collect::<CommandResult<Vec<_>>>()?;
        storage.set_criteria(task.id, &edits)?;
    }
    storage.update_task(&task)?;
    let task = storage
        .get_task(task.id)?
        .ok_or(StorageError::TaskNotFound(task.id))?;
    Ok((&task).into())
}

/// Checks (`met`) or unchecks a manual completion criterion of a task that is not done,
/// with what shows it; blank evidence is none. A task in progress whose criteria are then
/// all met awaits completion until it is completed.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn check_criterion(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    criterion_id: CriterionId,
    met: bool,
    evidence: Option<String>,
) -> CommandResult<Task> {
    check(
        &mut lock(&storage),
        task_id,
        criterion_id,
        met,
        evidence.as_deref(),
        CheckedBy::User,
    )
}

/// Checks a manual criterion as [`check_criterion`] does, as `by`.
pub(crate) fn check(
    storage: &mut Storage,
    task_id: TaskId,
    criterion_id: CriterionId,
    met: bool,
    evidence: Option<&str>,
    by: CheckedBy,
) -> CommandResult<Task> {
    let evidence = evidence
        .map(str::trim)
        .filter(|evidence| !evidence.is_empty());
    let task = storage.check_criterion(task_id.into(), criterion_id.into(), met, evidence, by)?;
    Ok((&task).into())
}

/// Pauses an `in_progress` or `in_review` task: it becomes `planned` again and keeps
/// its branch, worktree, herdr workspace, commit records, and pull request, so starting
/// it again resumes the work.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn pause_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    pause(&mut lock(&storage), id)
}

fn pause(storage: &mut Storage, id: TaskId) -> CommandResult<Task> {
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
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
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
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
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
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn add_dependency(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    super::herdr::add_dependencies(&storage, &herdr, task_id.into(), &[depends_on.into()])?;
    Ok(())
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn remove_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).remove_dependency(task_id.into(), depends_on.into())?)
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
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
    use crate::domain::path::WindowsPath;
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
        let error = edit_task(&mut storage, edit(task.id.into(), " ")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = edit_task(&mut storage, edit(TaskId(999), "Other")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(storage.get_task(task.id).unwrap().unwrap(), task);
    }
}
