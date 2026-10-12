//! Commands that drive herdr: the server health check, starting a task in a new
//! worktree, and focusing the herdr workspace of a started task.
//!
//! They run on the async runtime instead of the main thread because herdr calls can take
//! seconds. The database lock is not held while herdr runs.

use tauri::State;

use super::types::{HerdrStatus, Task, TaskId};
use super::{CommandError, CommandResult, HerdrState, StorageState, lock};
use crate::adapters::herdr::{Herdr, NewWorktree};
use crate::domain::model::{self, Machine, WorkspaceLocation};
use crate::domain::naming;
use crate::domain::status::TaskStatus;
use crate::storage::StorageError;

#[tauri::command(async)]
#[specta::specta]
pub fn herdr_status(herdr: State<'_, HerdrState>) -> CommandResult<HerdrStatus> {
    Ok((&herdr.server_status()?).into())
}

/// Creates the worktree of a `ready` task through herdr and moves the task to
/// `in_progress`. Requires a worktree root for the machine of the task's workspace.
#[tauri::command(async)]
#[specta::specta]
pub fn start_task(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&start(&storage, &herdr, id.into())?).into())
}

#[tauri::command(async)]
#[specta::specta]
pub fn open_task_in_herdr(
    storage: State<'_, StorageState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<()> {
    open(&storage, &herdr, id.into())
}

fn start(storage: &StorageState, herdr: &Herdr, id: model::TaskId) -> CommandResult<model::Task> {
    let (workspace, checkout) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        task.status
            .transition_to(TaskStatus::InProgress)
            .map_err(StorageError::from)?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        let settings = storage.get_machine_settings(&workspace.location.machine())?;
        let checkout = naming::task_checkout(&workspace, &settings, &task)?;
        (workspace, checkout)
    };
    let repository = match &workspace.location {
        WorkspaceLocation::Local { path } => path.as_str(),
        WorkspaceLocation::Ssh { path, .. } => path.as_str(),
    };
    let path = checkout.path.to_string();
    let herdr_workspace_id = herdr.create_worktree(
        &workspace.location.machine(),
        &NewWorktree {
            repository,
            path: &path,
            branch: &checkout.branch,
            base: &workspace.default_base_branch,
        },
    )?;
    Ok(lock(storage).start_task(id, &checkout, &herdr_workspace_id)?)
}

fn open(storage: &StorageState, herdr: &Herdr, id: model::TaskId) -> CommandResult<()> {
    let (machine, herdr_workspace_id): (Machine, String) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        let herdr_workspace_id = task.herdr_workspace_id.ok_or_else(|| {
            CommandError::rejected(format!("task {id} has no herdr workspace; start it first"))
        })?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        (workspace.location.machine(), herdr_workspace_id)
    };
    Ok(herdr.focus_workspace(&machine, &herdr_workspace_id)?)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::adapters::git::fake::FakeRunner;
    use crate::adapters::git::{GitError, Invocation};
    use crate::commands::CommandErrorKind;
    use crate::domain::model::{HostPath, MachineLabel, MachineSettings, TaskType};
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::storage::{NewTask, NewWorkspace, Storage};

    const CREATED: &str =
        r#"{"id":"cli:worktree:create","result":{"workspace":{"workspace_id":"w27"}}}"#;

    struct Fixture {
        storage: StorageState,
        task: model::Task,
    }

    fn fixture(location: WorkspaceLocation, settings: Option<MachineSettings>) -> Fixture {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "Toodo".to_owned(),
                location,
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let task = storage
            .create_task(&NewTask {
                workspace_id: workspace.id,
                task_type: TaskType::Feat,
                title: "Add login".to_owned(),
                description: String::new(),
            })
            .unwrap();
        if let Some(settings) = settings {
            storage.save_machine_settings(&settings).unwrap();
        }
        Fixture {
            storage: Mutex::new(storage),
            task,
        }
    }

    fn local_fixture(root: Option<&str>) -> Fixture {
        fixture(
            WorkspaceLocation::Local {
                path: WindowsPath::parse(r"C:\dev\toodo").unwrap(),
            },
            Some(MachineSettings::Local {
                worktree_root: root.map(|root| WindowsPath::parse(root).unwrap()),
            }),
        )
    }

    fn herdr(results: Vec<Result<String, GitError>>) -> (Herdr, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (Herdr::new(Box::new(runner.clone())), runner)
    }

    fn invocations(runner: &FakeRunner) -> Vec<Invocation> {
        runner.invocations.lock().unwrap().clone()
    }

    fn stored(fixture: &Fixture) -> model::Task {
        lock(&fixture.storage)
            .get_task(fixture.task.id)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn a_ready_task_starts_in_a_new_herdr_worktree() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);

        let started = start(&fixture.storage, &herdr, fixture.task.id).unwrap();

        assert_eq!(started.status, TaskStatus::InProgress);
        assert_eq!(started.branch.as_deref(), Some("feat/add-login-1"));
        assert_eq!(
            started.worktree_path,
            Some(HostPath::Windows(
                WindowsPath::parse(r"D:\wt\toodo\feat-add-login-1").unwrap()
            ))
        );
        assert_eq!(started.herdr_workspace_id.as_deref(), Some("w27"));
        assert_eq!(stored(&fixture), started);
        assert_eq!(
            invocations(&runner)[0].args,
            [
                "worktree",
                "create",
                "--cwd",
                r"C:\dev\toodo",
                "--path",
                r"D:\wt\toodo\feat-add-login-1",
                "--branch",
                "feat/add-login-1",
                "--base",
                "main",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn ssh_tasks_start_on_their_machine() {
        let devbox = MachineLabel::parse("devbox").unwrap();
        let fixture = fixture(
            WorkspaceLocation::Ssh {
                machine: devbox.clone(),
                path: PosixPath::parse("/srv/toodo").unwrap(),
            },
            Some(MachineSettings::Ssh {
                machine: devbox,
                worktree_root: Some(PosixPath::parse("/wt").unwrap()),
            }),
        );
        let (herdr, runner) = herdr(vec![
            Ok(
                r#"[{"id":"m7","label":"devbox","target":"devbox","session":"default",
                "enabled":true,"selected":false}]"#
                    .to_owned(),
            ),
            Ok(CREATED.to_owned()),
        ]);

        start(&fixture.storage, &herdr, fixture.task.id).unwrap();

        assert_eq!(
            invocations(&runner)[1].args[..7],
            [
                "--machine",
                "m7",
                "worktree",
                "create",
                "--cwd",
                "/srv/toodo",
                "--path"
            ]
        );
        assert_eq!(
            invocations(&runner)[1].args[7],
            "/wt/toodo/feat-add-login-1"
        );
    }

    #[test]
    fn a_missing_worktree_root_is_reported_before_herdr_runs() {
        let fixture = local_fixture(None);
        let (herdr, runner) = herdr(Vec::new());

        let error = start(&fixture.storage, &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        assert_eq!(error.message, "worktree root is not set for machine local");
        assert!(invocations(&runner).is_empty());
    }

    #[test]
    fn a_task_that_is_not_ready_is_rejected_before_herdr_runs() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);
        start(&fixture.storage, &herdr, fixture.task.id).unwrap();

        let error = start(&fixture.storage, &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert_eq!(invocations(&runner).len(), 1);
    }

    #[test]
    fn a_herdr_failure_leaves_the_task_ready() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, _) = herdr(vec![Err(GitError::Failed {
            command: "herdr worktree create".to_owned(),
            stderr: r#"{"error":{"code":"worktree_create_failed","message":"fatal: invalid reference: main"}}"#.to_owned(),
        })]);

        let error = start(&fixture.storage, &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::External);
        assert!(error.message.ends_with("fatal: invalid reference: main"));
        assert_eq!(stored(&fixture), fixture.task);
    }

    #[test]
    fn open_focuses_the_herdr_workspace_of_a_started_task() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned()), Ok(String::new())]);

        let error = open(&fixture.storage, &herdr, fixture.task.id).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);

        start(&fixture.storage, &herdr, fixture.task.id).unwrap();
        open(&fixture.storage, &herdr, fixture.task.id).unwrap();
        assert_eq!(
            invocations(&runner)[1].args,
            ["workspace", "focus", "w27"].map(str::to_owned)
        );
    }
}
