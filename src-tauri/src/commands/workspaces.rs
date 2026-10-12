//! Commands for workspaces and per-machine settings.

use tauri::State;

use super::types::{
    self, MachineSettings, NewWorkspace, Workspace, WorkspaceId, WorkspaceSetup, WorkspaceSource,
};
use super::{CommandError, CommandResult, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::adapters::herdr::Herdr;
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

/// Clones the repository or checks the existing one, opens it as a herdr workspace, then
/// saves the workspace. Git and herdr run on a blocking thread because a clone can take
/// minutes.
#[tauri::command]
#[specta::specta]
pub async fn create_workspace(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    herdr: State<'_, HerdrState>,
    workspace: NewWorkspace,
) -> CommandResult<Workspace> {
    let setup = workspace.try_into()?;
    let git = GitState::clone(&git);
    let herdr = HerdrState::clone(&herdr);
    let new = tauri::async_runtime::spawn_blocking(move || prepare_workspace(&git, &herdr, setup))
        .await
        .map_err(|error| CommandError::internal(format!("workspace setup panicked: {error}")))??;
    Ok((&lock(&storage).create_workspace(&new)?).into())
}

/// Brings the repository to the workspace location, resolves the base branch, and opens
/// the repository in herdr unless a herdr workspace already has it open.
fn prepare_workspace(
    git: &Git,
    herdr: &Herdr,
    setup: WorkspaceSetup,
) -> CommandResult<storage::NewWorkspace> {
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
    let repository = match &location {
        model::WorkspaceLocation::Local { path } => path.as_str(),
        model::WorkspaceLocation::Ssh { path, .. } => path.as_str(),
    };
    herdr.open_workspace(&location.machine(), repository, &setup.name)?;
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

    /// `herdr workspace list` output without the repository, then `herdr workspace create`.
    fn herdr_opens_a_new_workspace() -> Vec<Result<String, GitError>> {
        vec![
            Ok(r#"{"id":"cli:workspace:list","result":{"workspaces":[]}}"#.to_owned()),
            Ok(
                r#"{"id":"cli:workspace:create","result":{"workspace":{"workspace_id":"w9"}}}"#
                    .to_owned(),
            ),
        ]
    }

    struct Prepared {
        result: CommandResult<storage::NewWorkspace>,
        invocations: Vec<Invocation>,
        herdr_invocations: Vec<Invocation>,
    }

    fn prepare(
        setup: WorkspaceSetup,
        results: Vec<Result<String, GitError>>,
        herdr_results: Vec<Result<String, GitError>>,
    ) -> Prepared {
        let runner = Arc::new(FakeRunner::new(results));
        let herdr_runner = Arc::new(FakeRunner::new(herdr_results));
        let result = prepare_workspace(
            &Git::new(Box::new(runner.clone())),
            &Herdr::new(Box::new(herdr_runner.clone())),
            setup,
        );
        Prepared {
            result,
            invocations: runner.invocations.lock().unwrap().clone(),
            herdr_invocations: herdr_runner.invocations.lock().unwrap().clone(),
        }
    }

    #[test]
    fn open_uses_the_repository_root_and_detects_the_base_branch() {
        let prepared = prepare(
            setup(WorkspaceSource::Open, None),
            vec![
                Ok("C:/dev/toodo\n".to_owned()),
                Ok("origin/main\n".to_owned()),
            ],
            herdr_opens_a_new_workspace(),
        );
        let new = prepared.result.unwrap();
        assert_eq!(new.location, local(r"C:\dev\toodo"));
        assert_eq!(new.default_base_branch, "main");
        assert_eq!(prepared.invocations[1].args[..2], ["-C", r"C:\dev\toodo"]);
        assert_eq!(
            prepared.herdr_invocations[1].args,
            [
                "workspace",
                "create",
                "--cwd",
                r"C:\dev\toodo",
                "--label",
                "toodo",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn open_rejects_a_path_outside_a_repository() {
        let prepared = prepare(
            setup(WorkspaceSource::Open, None),
            vec![Err(failed())],
            Vec::new(),
        );
        assert_eq!(
            prepared.result.unwrap_err().kind,
            CommandErrorKind::External
        );
        assert_eq!(prepared.invocations.len(), 1);
        assert!(prepared.herdr_invocations.is_empty());
    }

    #[test]
    fn clone_keeps_the_given_location_and_base_branch() {
        let source = WorkspaceSource::Clone {
            url: "git@h:a.git".to_owned(),
        };
        let prepared = prepare(
            setup(source, Some("develop")),
            vec![Ok(String::new())],
            herdr_opens_a_new_workspace(),
        );
        let new = prepared.result.unwrap();
        assert_eq!(new.location, local(r"C:\dev\toodo\src"));
        assert_eq!(new.default_base_branch, "develop");
        assert_eq!(prepared.invocations[0].args[0], "clone");
        assert_eq!(prepared.invocations.len(), 1);
        assert_eq!(prepared.herdr_invocations[1].args[3], r"C:\dev\toodo\src");
    }

    #[test]
    fn a_herdr_failure_does_not_save_the_workspace() {
        let prepared = prepare(
            setup(WorkspaceSource::Open, Some("main")),
            vec![Ok("C:/dev/toodo\n".to_owned())],
            vec![Err(GitError::Spawn {
                program: "herdr".to_owned(),
                message: "program not found".to_owned(),
            })],
        );
        assert_eq!(
            prepared.result.unwrap_err().kind,
            CommandErrorKind::External
        );
    }

    #[test]
    fn undetectable_base_branch_asks_for_one() {
        let prepared = prepare(
            setup(WorkspaceSource::Open, None),
            vec![
                Ok("C:/dev/toodo\n".to_owned()),
                Err(failed()),
                Err(failed()),
            ],
            Vec::new(),
        );
        assert_eq!(
            prepared.result.unwrap_err().kind,
            CommandErrorKind::InvalidInput
        );
        assert!(prepared.herdr_invocations.is_empty());
    }
}
