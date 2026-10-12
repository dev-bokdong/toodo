//! Commands for tasks, their dependency DAG, and their commit records.

use tauri::State;

use super::types::{
    self, CommitRecord, NewTask, StatusChange, Task, TaskEdit, TaskId, TaskStatus, WorkspaceId,
};
use super::{CommandError, CommandResult, StorageState, lock};
use crate::storage::{Storage, StorageError};

/// Tasks of a workspace ordered by ID, each with its dependencies.
#[tauri::command]
#[specta::specta]
pub fn list_tasks(
    storage: State<'_, StorageState>,
    workspace_id: WorkspaceId,
) -> CommandResult<Vec<Task>> {
    Ok(types::map_all(
        &lock(&storage).list_tasks(workspace_id.into())?,
    ))
}

#[tauri::command]
#[specta::specta]
pub fn get_task(storage: State<'_, StorageState>, id: TaskId) -> CommandResult<Task> {
    let task = lock(&storage)
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

/// Creates a task without dependencies, so it starts out `ready`.
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

/// Moves a task to `in_progress`, `in_review`, `done`, or `cancelled` and returns the readiness
/// changes this caused in other tasks.
#[tauri::command]
#[specta::specta]
pub fn set_task_status(
    storage: State<'_, StorageState>,
    id: TaskId,
    status: TaskStatus,
) -> CommandResult<Vec<StatusChange>> {
    let changes = lock(&storage).set_task_status(id.into(), status.into())?;
    Ok(types::map_all(&changes))
}

/// Deletes a task with its dependency edges and commit records and returns the
/// readiness changes this caused in its dependents.
#[tauri::command]
#[specta::specta]
pub fn delete_task(
    storage: State<'_, StorageState>,
    id: TaskId,
) -> CommandResult<Vec<StatusChange>> {
    Ok(types::map_all(&lock(&storage).delete_task(id.into())?))
}

/// Makes `task_id` depend on `depends_on`. Rejects self, cross-workspace, and cyclic
/// edges.
#[tauri::command]
#[specta::specta]
pub fn add_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<Vec<StatusChange>> {
    let changes = lock(&storage).add_dependency(task_id.into(), depends_on.into())?;
    Ok(types::map_all(&changes))
}

#[tauri::command]
#[specta::specta]
pub fn remove_dependency(
    storage: State<'_, StorageState>,
    task_id: TaskId,
    depends_on: TaskId,
) -> CommandResult<Vec<StatusChange>> {
    let changes = lock(&storage).remove_dependency(task_id.into(), depends_on.into())?;
    Ok(types::map_all(&changes))
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
