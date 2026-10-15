//! What the MCP tools do, over the same storage as the Tauri commands, so agents follow
//! the rules the UI follows: the dependency DAG stays acyclic and readiness stays in sync.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::tasks::edit_task;
use crate::commands::types::{self, Task, TaskEdit, TaskId, TaskType, Workspace, WorkspaceId};
use crate::commands::{CommandError, CommandResult};
use crate::domain::model::NoteKind;
use crate::storage::{self, Storage, StorageError};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ListTasks {
    /// Only tasks of this workspace; all workspaces when omitted.
    pub workspace_id: Option<WorkspaceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CurrentTask {
    /// Absolute path of the agent's working directory, e.g. the worktree of its task.
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CreateTask {
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    /// Details in GitHub-flavored Markdown.
    #[serde(default)]
    pub description: String,
    /// Tasks the new task depends on, from any workspace.
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct UpdateTask {
    pub id: TaskId,
    /// Unchanged when omitted.
    pub task_type: Option<TaskType>,
    /// Unchanged when omitted.
    pub title: Option<String>,
    /// Details in GitHub-flavored Markdown; unchanged when omitted.
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AddDependencies {
    /// The dependent task.
    pub task_id: TaskId,
    /// Its prerequisites, from any workspace, which must all be done before `task_id` is
    /// ready.
    pub depends_on: Vec<TaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Dependency {
    /// The dependent task.
    pub task_id: TaskId,
    /// The prerequisite task, which must be done before `task_id` is ready.
    pub depends_on: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AddNote {
    /// The task the note is about, usually your current task.
    pub task_id: TaskId,
    /// What happened, in GitHub-flavored Markdown.
    pub note: String,
}

pub fn list_workspaces(storage: &Storage) -> CommandResult<Vec<Workspace>> {
    Ok(types::map_all(&storage.list_workspaces()?))
}

pub fn list_tasks(storage: &Storage, request: ListTasks) -> CommandResult<Vec<Task>> {
    let tasks = match request.workspace_id {
        Some(workspace) => {
            storage
                .get_workspace(workspace.into())?
                .ok_or(StorageError::WorkspaceNotFound(workspace.into()))?;
            storage.list_tasks(workspace.into())?
        }
        None => storage.list_all_tasks()?,
    };
    Ok(types::map_all(&tasks))
}

/// The task whose worktree contains `cwd`; the innermost one if worktrees are nested.
pub fn current_task(storage: &Storage, request: CurrentTask) -> CommandResult<Task> {
    let cwd = comparable_path(&request.cwd);
    storage
        .list_all_tasks()?
        .iter()
        .filter_map(|task| {
            let worktree = comparable_path(&task.worktree_path.as_ref()?.to_string());
            let inside = cwd == worktree
                || cwd
                    .strip_prefix(&worktree)
                    .is_some_and(|rest| rest.starts_with('/'));
            inside.then_some((worktree.len(), task))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, task)| task.into())
        .ok_or_else(|| {
            CommandError::not_found(format!(
                "no Toodo task has a worktree containing {}",
                request.cwd
            ))
        })
}

/// A path with `/` separators and no trailing separator; Windows paths, which are
/// case-insensitive, are also lowercased.
fn comparable_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let path = path.trim_end_matches('/');
    let bytes = path.as_bytes();
    let windows = path.starts_with("//") || (bytes.len() >= 2 && bytes[1] == b':');
    if windows {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Creates a `planned` task with its dependencies. Unknown prerequisites are rejected
/// before anything is written; a new task has no dependents, so its edges cannot close
/// a cycle.
pub fn create_task(storage: &mut Storage, request: CreateTask) -> CommandResult<Task> {
    for &prerequisite in &request.depends_on {
        storage
            .get_task(prerequisite.into())?
            .ok_or(StorageError::TaskNotFound(prerequisite.into()))?;
    }
    let new: storage::NewTask = types::NewTask {
        workspace_id: request.workspace_id,
        task_type: request.task_type,
        title: request.title,
        description: request.description,
    }
    .try_into()?;
    let task = storage.create_task(&new)?;
    for &prerequisite in &request.depends_on {
        if let Err(error) = storage.add_dependency(task.id, prerequisite.into()) {
            if let Err(cleanup) = storage.delete_task(task.id) {
                eprintln!(
                    "toodo: failed to delete task {} after a failed MCP create: {cleanup}",
                    task.id
                );
            }
            return Err(error.into());
        }
    }
    get_task(storage, task.id.into())
}

pub fn update_task(storage: &mut Storage, request: UpdateTask) -> CommandResult<Task> {
    let current = get_task(storage, request.id)?;
    edit_task(
        storage,
        TaskEdit {
            id: request.id,
            task_type: request.task_type.unwrap_or(current.task_type),
            title: request.title.unwrap_or(current.title),
            description: request.description.unwrap_or(current.description),
        },
    )
}

/// Adds the edges in one step and returns the dependent task. Self and cyclic edges are
/// rejected, and one rejected edge adds none. An `in_progress` task that a prerequisite
/// blocks is paused; its herdr workspace stays open, and the app asks the user whether to
/// close it, since that ends the agent's session.
pub fn add_dependencies(storage: &mut Storage, request: AddDependencies) -> CommandResult<Task> {
    if request.depends_on.is_empty() {
        return Err(CommandError::invalid_input(
            "depends_on must name at least one task",
        ));
    }
    let depends_on: Vec<_> = request.depends_on.iter().map(|&id| id.into()).collect();
    let added = storage.add_dependencies(request.task_id.into(), &depends_on)?;
    Ok((&added.task).into())
}

pub fn remove_dependency(storage: &mut Storage, request: Dependency) -> CommandResult<Task> {
    storage.remove_dependency(request.task_id.into(), request.depends_on.into())?;
    get_task(storage, request.task_id)
}

/// Appends a note of `kind` to the task's history and returns the task with its notes.
pub fn add_note(storage: &mut Storage, kind: NoteKind, request: AddNote) -> CommandResult<Task> {
    let note = request.note.trim();
    if note.is_empty() {
        return Err(CommandError::invalid_input("note must not be empty"));
    }
    let task = storage.add_task_note(request.task_id.into(), kind, note)?;
    Ok((&task).into())
}

fn get_task(storage: &Storage, id: TaskId) -> CommandResult<Task> {
    let task = storage
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;
    use crate::domain::model::{self, HostPath, WorkspaceLocation};
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::storage::NewWorkspace;

    fn storage_with_workspace() -> (Storage, WorkspaceId) {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "toodo".to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        (storage, workspace.id.into())
    }

    fn create(
        storage: &mut Storage,
        workspace: WorkspaceId,
        title: &str,
        depends_on: Vec<TaskId>,
    ) -> CommandResult<Task> {
        create_task(
            storage,
            CreateTask {
                workspace_id: workspace,
                task_type: TaskType::Feat,
                title: title.to_owned(),
                description: String::new(),
                depends_on,
            },
        )
    }

    fn set_worktree(storage: &mut Storage, id: TaskId, path: HostPath) {
        let mut task = storage.get_task(id.into()).unwrap().unwrap();
        task.worktree_path = Some(path);
        storage.update_task(&task).unwrap();
    }

    #[test]
    fn creates_a_task_with_its_dependencies() {
        let (mut storage, workspace) = storage_with_workspace();
        let first = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let second = create(&mut storage, workspace, "API", vec![first.id]).unwrap();
        assert_eq!(second.depends_on, vec![first.id]);
        assert_eq!(second.status, types::TaskStatus::Planned);
        let stored = storage.get_task(second.id.into()).unwrap().unwrap();
        assert_eq!(Task::from(&stored), second);
    }

    #[test]
    fn an_unknown_prerequisite_or_empty_title_creates_nothing() {
        let (mut storage, workspace) = storage_with_workspace();
        let error = create(&mut storage, workspace, "API", vec![TaskId(42)]).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        let error = create(&mut storage, workspace, "  ", vec![]).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        assert!(storage.list_all_tasks().unwrap().is_empty());
    }

    #[test]
    fn a_dependency_that_closes_a_cycle_is_rejected() {
        let (mut storage, workspace) = storage_with_workspace();
        let first = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let second = create(&mut storage, workspace, "API", vec![first.id]).unwrap();
        let request = AddDependencies {
            task_id: first.id,
            depends_on: vec![second.id],
        };
        let error = add_dependencies(&mut storage, request.clone()).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("cycle"), "{}", error.message);
        assert!(get_task(&storage, first.id).unwrap().depends_on.is_empty());

        let removed = remove_dependency(
            &mut storage,
            Dependency {
                task_id: second.id,
                depends_on: first.id,
            },
        )
        .unwrap();
        assert!(removed.depends_on.is_empty());
        assert_eq!(
            add_dependencies(&mut storage, request).unwrap().depends_on,
            vec![second.id]
        );
    }

    #[test]
    fn adds_several_prerequisites_at_once_and_needs_at_least_one() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Verify", vec![]).unwrap();
        let fix = create(&mut storage, workspace, "Fix login", vec![]).unwrap();
        let docs = create(&mut storage, workspace, "Fix docs", vec![]).unwrap();

        let error = add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let updated = add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![fix.id, docs.id],
            },
        )
        .unwrap();
        assert_eq!(updated.depends_on, vec![fix.id, docs.id]);
    }

    #[test]
    fn a_blocking_prerequisite_pauses_the_task_but_leaves_its_herdr_workspace_open() {
        use crate::domain::status::TaskStatus;
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Verify", vec![]).unwrap();
        let fix = create(&mut storage, workspace, "Fix login", vec![]).unwrap();
        storage
            .set_task_status(task.id.into(), TaskStatus::InProgress)
            .unwrap();
        storage
            .set_task_herdr_workspace(task.id.into(), "w1")
            .unwrap();

        add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![fix.id],
            },
        )
        .unwrap();

        let paused = storage.get_task(task.id.into()).unwrap().unwrap();
        assert_eq!(paused.status, TaskStatus::Planned);
        assert_eq!(paused.herdr_workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn update_changes_only_the_given_fields() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let updated = update_task(
            &mut storage,
            UpdateTask {
                id: task.id,
                task_type: None,
                title: None,
                description: Some("Add tables".to_owned()),
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Schema");
        assert_eq!(updated.task_type, TaskType::Feat);
        assert_eq!(updated.description, "Add tables");
    }

    #[test]
    fn notes_are_trimmed_and_a_blank_note_or_unknown_task_adds_none() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let note = |task_id: TaskId, note: &str| AddNote {
            task_id,
            note: note.to_owned(),
        };

        let noted = add_note(
            &mut storage,
            NoteKind::Progress,
            note(task.id, "  Chose SQLite \n"),
        )
        .unwrap();
        let noted = add_note(&mut storage, NoteKind::Blocker, note(noted.id, "No access")).unwrap();
        let notes: Vec<_> = noted
            .notes
            .iter()
            .map(|note| (note.kind, note.body.as_str()))
            .collect();
        assert_eq!(
            notes,
            [
                (types::NoteKind::Progress, "Chose SQLite"),
                (types::NoteKind::Blocker, "No access"),
            ]
        );

        let error = add_note(&mut storage, NoteKind::Blocker, note(task.id, " \n")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = add_note(&mut storage, NoteKind::Progress, note(TaskId(99), "x")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(get_task(&storage, task.id).unwrap().notes.len(), 2);
    }

    #[test]
    fn lists_tasks_of_one_workspace_or_all() {
        let (mut storage, workspace) = storage_with_workspace();
        create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let all = list_tasks(&storage, ListTasks { workspace_id: None }).unwrap();
        assert_eq!(all.len(), 1);
        let error = list_tasks(
            &storage,
            ListTasks {
                workspace_id: Some(WorkspaceId(99)),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
    }

    #[test]
    fn the_current_task_is_the_one_whose_worktree_contains_the_cwd() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        create(&mut storage, workspace, "Other", vec![]).unwrap();
        set_worktree(
            &mut storage,
            task.id,
            HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-schema-1").unwrap()),
        );
        let find = |cwd: &str| {
            current_task(
                &storage,
                CurrentTask {
                    cwd: cwd.to_owned(),
                },
            )
        };

        assert_eq!(find(r"D:\wt\toodo\feat-schema-1").unwrap().id, task.id);
        assert_eq!(find("d:/WT/toodo/feat-schema-1/src/").unwrap().id, task.id);
        let error = find(r"D:\wt\toodo\feat-schema-10").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert!(find(r"D:\wt\toodo").is_err());
    }

    #[test]
    fn posix_worktrees_are_matched_case_sensitively() {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "server".to_owned(),
                location: WorkspaceLocation::Ssh {
                    machine: model::MachineLabel::parse("devbox").unwrap(),
                    path: PosixPath::parse("/srv/app").unwrap(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let task = create(&mut storage, workspace.id.into(), "Remote", vec![]).unwrap();
        set_worktree(
            &mut storage,
            task.id,
            HostPath::Posix(PosixPath::parse("/wt/app/feat-remote-1").unwrap()),
        );
        let find = |cwd: &str| {
            current_task(
                &storage,
                CurrentTask {
                    cwd: cwd.to_owned(),
                },
            )
        };
        assert_eq!(find("/wt/app/feat-remote-1/src").unwrap().id, task.id);
        assert!(find("/WT/app/feat-remote-1").is_err());
    }
}
