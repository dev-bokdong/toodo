//! Commands for workspaces and per-machine settings.

use tauri::State;

use super::types::{
    self, MachineSettings, NewWorkspace, Workspace, WorkspaceId, WorkspaceSetup, WorkspaceSource,
};
use super::{CommandError, CommandResult, GitState, StorageState, lock};
use crate::adapters::git::Git;
use crate::domain::model;
use crate::storage::{self, StorageError};

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

/// Clones the repository or checks the existing one, then saves the workspace. Git runs
/// on a blocking thread because a clone can take minutes.
#[tauri::command]
#[specta::specta]
pub async fn create_workspace(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    workspace: NewWorkspace,
) -> CommandResult<Workspace> {
    let setup = workspace.try_into()?;
    let git = GitState::clone(&git);
    let new = tauri::async_runtime::spawn_blocking(move || prepare_workspace(&git, setup))
        .await
        .map_err(|error| CommandError::internal(format!("workspace setup panicked: {error}")))??;
    Ok((&lock(&storage).create_workspace(&new)?).into())
}

/// Brings the repository to the workspace location and resolves the base branch.
fn prepare_workspace(git: &Git, setup: WorkspaceSetup) -> CommandResult<storage::NewWorkspace> {
    let location = match setup.source {
        WorkspaceSource::Clone { url } => {
            git.clone_repository(&url, &setup.location)?;
            setup.location
        }
        WorkspaceSource::Open => git.repository_root(&setup.location)?,
    };
    let default_base_branch = match setup.default_base_branch {
        Some(branch) => branch,
        None => git.default_branch(&location)?.ok_or_else(|| {
            CommandError::invalid_input(
                "could not detect the default branch of the repository; enter a base branch",
            )
        })?,
    };
    Ok(storage::NewWorkspace {
        name: setup.name,
        location,
        default_base_branch,
    })
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::fake::{FakeRunner, failed};
    use crate::adapters::git::{GitError, Invocation};
    use crate::commands::CommandErrorKind;
    use crate::domain::path::WindowsPath;

    fn local(path: &str) -> model::WorkspaceLocation {
        model::WorkspaceLocation::Local {
            path: WindowsPath::parse(path).unwrap(),
        }
    }

    fn setup(source: WorkspaceSource, branch: Option<&str>) -> WorkspaceSetup {
        WorkspaceSetup {
            name: "toodo".to_owned(),
            location: local(r"C:\dev\toodo\src"),
            source,
            default_base_branch: branch.map(str::to_owned),
        }
    }

    fn prepare(
        setup: WorkspaceSetup,
        results: Vec<Result<String, GitError>>,
    ) -> (CommandResult<storage::NewWorkspace>, Vec<Invocation>) {
        let runner = Arc::new(FakeRunner::new(results));
        let result = prepare_workspace(&Git::new(Box::new(runner.clone())), setup);
        let invocations = runner.invocations.lock().unwrap().clone();
        (result, invocations)
    }

    #[test]
    fn open_uses_the_repository_root_and_detects_the_base_branch() {
        let (result, invocations) = prepare(
            setup(WorkspaceSource::Open, None),
            vec![
                Ok("C:/dev/toodo\n".to_owned()),
                Ok("origin/main\n".to_owned()),
            ],
        );
        let new = result.unwrap();
        assert_eq!(new.location, local(r"C:\dev\toodo"));
        assert_eq!(new.default_base_branch, "main");
        assert_eq!(invocations[1].args[..2], ["-C", r"C:\dev\toodo"]);
    }

    #[test]
    fn open_rejects_a_path_outside_a_repository() {
        let (result, invocations) =
            prepare(setup(WorkspaceSource::Open, None), vec![Err(failed())]);
        assert_eq!(result.unwrap_err().kind, CommandErrorKind::External);
        assert_eq!(invocations.len(), 1);
    }

    #[test]
    fn clone_keeps_the_given_location_and_base_branch() {
        let source = WorkspaceSource::Clone {
            url: "git@h:a.git".to_owned(),
        };
        let (result, invocations) =
            prepare(setup(source, Some("develop")), vec![Ok(String::new())]);
        let new = result.unwrap();
        assert_eq!(new.location, local(r"C:\dev\toodo\src"));
        assert_eq!(new.default_base_branch, "develop");
        assert_eq!(invocations[0].args[0], "clone");
        assert_eq!(invocations.len(), 1);
    }

    #[test]
    fn undetectable_base_branch_asks_for_one() {
        let (result, _) = prepare(
            setup(WorkspaceSource::Open, None),
            vec![
                Ok("C:/dev/toodo\n".to_owned()),
                Err(failed()),
                Err(failed()),
            ],
        );
        assert_eq!(result.unwrap_err().kind, CommandErrorKind::InvalidInput);
    }
}
