//! Commands for workspaces and per-machine settings.

use tauri::State;

use super::types::{self, MachineSettings, NewWorkspace, Workspace, WorkspaceId};
use super::{CommandResult, StorageState, lock};
use crate::domain::model;
use crate::storage::StorageError;

#[tauri::command]
#[specta::specta]
pub fn list_workspaces(storage: State<'_, StorageState>) -> CommandResult<Vec<Workspace>> {
    Ok(types::map_all(&lock(&storage).list_workspaces()?))
}

#[tauri::command]
#[specta::specta]
pub fn get_workspace(
    storage: State<'_, StorageState>,
    id: WorkspaceId,
) -> CommandResult<Workspace> {
    let workspace = lock(&storage)
        .get_workspace(id.into())?
        .ok_or(StorageError::WorkspaceNotFound(id.into()))?;
    Ok((&workspace).into())
}

#[tauri::command]
#[specta::specta]
pub fn create_workspace(
    storage: State<'_, StorageState>,
    workspace: NewWorkspace,
) -> CommandResult<Workspace> {
    let new = workspace.try_into()?;
    Ok((&lock(&storage).create_workspace(&new)?).into())
}

/// Saves the name, location, and default base branch of an existing workspace.
#[tauri::command]
#[specta::specta]
pub fn update_workspace(
    storage: State<'_, StorageState>,
    workspace: Workspace,
) -> CommandResult<Workspace> {
    let workspace: model::Workspace = workspace.try_into()?;
    lock(&storage).update_workspace(&workspace)?;
    Ok((&workspace).into())
}

/// Deletes a workspace together with its tasks, dependencies, and commit records.
#[tauri::command]
#[specta::specta]
pub fn delete_workspace(storage: State<'_, StorageState>, id: WorkspaceId) -> CommandResult<()> {
    Ok(lock(&storage).delete_workspace(id.into())?)
}

#[tauri::command]
#[specta::specta]
pub fn get_machine_settings(
    storage: State<'_, StorageState>,
    machine: types::Machine,
) -> CommandResult<MachineSettings> {
    let machine = machine.try_into()?;
    Ok((&lock(&storage).get_machine_settings(&machine)?).into())
}

#[tauri::command]
#[specta::specta]
pub fn save_machine_settings(
    storage: State<'_, StorageState>,
    settings: MachineSettings,
) -> CommandResult<MachineSettings> {
    let settings: model::MachineSettings = settings.try_into()?;
    lock(&storage).save_machine_settings(&settings)?;
    Ok((&settings).into())
}
