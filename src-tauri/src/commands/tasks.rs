//! Commands for tasks, their dependency DAG, and their commit records.

use tauri::State;

use super::types::{self, CommitRecord, NewTask, Task, TaskEdit, TaskId};
use super::{CommandError, CommandResult, HerdrState, StorageState, lock};
use crate::domain::status::TaskStatus;
use crate::storage::{Storage, StorageError};

/// Tasks of every workspace ordered by ID, each with its dependencies, which may belong
/// to another workspace.
#[tauri::command]
#[specta::specta]
pub fn list_tasks(storage: State<'_, StorageState>) -> CommandResult<Vec<Task>> {
    Ok(types::map_all(&lock(&storage).list_all_tasks()?))
}

#[tauri::command]
#[specta::specta]
pub fn get_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    let task = lock(&storage)
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

/// Creates a `planned` task without dependencies.
#[tauri::command]
#[specta::specta]
pub fn create_task(storage: State<'_, StorageState>, task: NewTask) -> CommandResult<Task> {
    let new = task.try_into()?;
    Ok((&lock(&storage).create_task(&new)?).into())
}

#[tauri::command]
#[specta::specta]
pub fn update_task(storage: State<'_, StorageState>, task: TaskEdit) -> CommandResult<Task> {
    edit_task(&mut lock(&storage), task)
}

fn edit_task(storage: &mut Storage, edit: TaskEdit) -> CommandResult<Task> {
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
    storage.update_task(&task)?;
    Ok((&task).into())
}

/// Pauses an `in_progress` or `in_review` task: it becomes `planned` again and keeps
/// its branch, worktree, herdr workspace, commit records, and pull request, so starting
/// it again resumes the work.
#[tauri::command]
#[specta::specta]
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

/// Deletes a task with its dependency edges and commit records. A task that still has a worktree
/// first removes it through herdr, which also closes its herdr workspace; the branch is
/// kept. Without `force`, a worktree with modified or untracked files is rejected and the
/// task is kept.
#[tauri::command(async)]
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
/// self and cyclic edges.
#[tauri::command]
#[specta::specta]
pub fn add_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).add_dependency(task_id.into(), depends_on.into())?)
}

#[tauri::command]
#[specta::specta]
pub fn remove_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<()> {
    Ok(lock(&storage).remove_dependency(task_id.into(), depends_on.into())?)
}

#[tauri::command]
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
    use crate::domain::path::WindowsPath;
    use crate::storage::{NewTask, NewWorkspace};

    fn storage_with_task() -> (Storage, model::Task) {
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
        let mut task = storage
            .create_task(&NewTask {
                workspace_id: workspace.id,
                task_type: model::TaskType::Feat,
                title: "Add commands".to_owned(),
                description: String::new(),
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
            },
        )
        .unwrap();
        assert_eq!(edited.title, "Fix commands");
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
    fn edit_task_rejects_empty_title_and_unknown_task() {
        let (mut storage, task) = storage_with_task();
        let edit = |id: TaskId, title: &str| TaskEdit {
            id,
            task_type: types::TaskType::Feat,
            title: title.to_owned(),
            description: String::new(),
        };
        let error = edit_task(&mut storage, edit(task.id.into(), " ")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = edit_task(&mut storage, edit(TaskId(999), "Other")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(storage.get_task(task.id).unwrap().unwrap(), task);
    }
}
