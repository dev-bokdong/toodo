//! Commands that drive herdr: the server health check, starting a task in a new
//! worktree, focusing the herdr workspace of a started task, restoring the worktree of an
//! active task that lost it, and deleting a task together with its worktree.
//!
//! They run on the async runtime instead of the main thread because herdr calls can take
//! seconds. The database lock is not held while herdr runs.

use tauri::State;

use super::types::{HerdrStatus, Task, TaskId};
use super::{CommandError, CommandResult, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::adapters::herdr::{Herdr, HerdrError, NewWorktree};
use crate::domain::dag::StatusChange;
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
/// An empty repository first gets an initial commit on the workspace's base branch.
#[tauri::command(async)]
#[specta::specta]
pub fn start_task(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&start(&storage, &git, &herdr, id.into())?).into())
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

/// Checks out the branch of an `in_progress` or `in_review` task that lost its worktree,
/// e.g. because it was removed in herdr, into a new worktree opened in herdr. The branch
/// is taken from `origin` when the repository no longer has it.
#[tauri::command(async)]
#[specta::specta]
pub fn restore_task_worktree(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&restore(&storage, &git, &herdr, id.into())?).into())
}

fn start(
    storage: &StorageState,
    git: &Git,
    herdr: &Herdr,
    id: model::TaskId,
) -> CommandResult<model::Task> {
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
    git.create_initial_commit(&workspace.location, &workspace.default_base_branch)?;
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

fn restore(
    storage: &StorageState,
    git: &Git,
    herdr: &Herdr,
    id: model::TaskId,
) -> CommandResult<model::Task> {
    let (workspace, checkout) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        if !matches!(task.status, TaskStatus::InProgress | TaskStatus::InReview) {
            return Err(CommandError::rejected(format!(
                "task {id} is {}; only in progress or in review tasks can restore their worktree",
                task.status
            )));
        }
        if let Some(path) = &task.worktree_path {
            return Err(CommandError::rejected(format!(
                "task {id} already has its worktree at {path}"
            )));
        }
        let branch = task
            .branch
            .clone()
            .ok_or_else(|| CommandError::rejected(format!("task {id} has no branch")))?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        let settings = storage.get_machine_settings(&workspace.location.machine())?;
        // The title may have changed since the task started; the path follows it, the
        // branch keeps its name.
        let mut checkout = naming::task_checkout(&workspace, &settings, &task)?;
        checkout.branch = branch;
        (workspace, checkout)
    };
    let base = git
        .branch_source(&workspace.location, &checkout.branch)?
        .ok_or_else(|| {
            CommandError::rejected(format!(
                "branch {} of task {id} no longer exists in the repository or on origin",
                checkout.branch
            ))
        })?;
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
            base: &base,
        },
    )?;
    Ok(lock(storage).restore_task_worktree(id, &checkout.path, &herdr_workspace_id)?)
}

/// Focuses the herdr workspace of a started task. When herdr no longer has it, e.g. the
/// user closed it, the task's worktree is opened again and the new workspace remembered.
fn open(storage: &StorageState, herdr: &Herdr, id: model::TaskId) -> CommandResult<()> {
    let (location, worktree_path, herdr_workspace_id) = {
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
        (workspace.location, task.worktree_path, herdr_workspace_id)
    };
    let machine: Machine = location.machine();
    match herdr.focus_workspace(&machine, &herdr_workspace_id) {
        Err(error) if has_code(&error, WORKSPACE_NOT_FOUND) => {
            let Some(path) = worktree_path else {
                return Err(error.into());
            };
            let repository = match &location {
                WorkspaceLocation::Local { path } => path.as_str(),
                WorkspaceLocation::Ssh { path, .. } => path.as_str(),
            };
            let reopened = herdr.open_worktree(&machine, repository, &path.to_string())?;
            lock(storage).set_task_herdr_workspace(id, &reopened)?;
            Ok(herdr.focus_workspace(&machine, &reopened)?)
        }
        result => Ok(result?),
    }
}

/// Deletes a task; a task that still has a worktree first removes it like
/// [`remove_worktree`], so a dirty worktree keeps the task unless `force` is set.
pub(super) fn delete(
    storage: &StorageState,
    herdr: &Herdr,
    id: model::TaskId,
    force: bool,
) -> CommandResult<Vec<StatusChange>> {
    let task = lock(storage)
        .get_task(id)?
        .ok_or(StorageError::TaskNotFound(id))?;
    if task.worktree_path.is_some() {
        remove_worktree(storage, herdr, id, force)?;
    }
    Ok(lock(storage).delete_task(id)?)
}

/// Removes the worktree of a `done` or `cancelled` task through herdr, which also closes
/// its herdr workspace, and forgets both on the task. The branch is kept. Without `force`,
/// a worktree with modified or untracked files is rejected.
fn remove_worktree(
    storage: &StorageState,
    herdr: &Herdr,
    id: model::TaskId,
    force: bool,
) -> CommandResult<model::Task> {
    let (location, path, herdr_workspace_id) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        if !matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled) {
            return Err(CommandError::rejected(format!(
                "task {id} is {}; only done or cancelled tasks can remove their worktree",
                task.status
            )));
        }
        let path = task
            .worktree_path
            .ok_or_else(|| CommandError::rejected(format!("task {id} has no worktree")))?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        (
            workspace.location,
            path.to_string(),
            task.herdr_workspace_id,
        )
    };
    let machine = location.machine();
    let dirty = |error: HerdrError| {
        if has_code(&error, DIRTY_WORKTREE) {
            CommandError::rejected(format!(
                "the worktree of task {id} at {path} has modified or untracked files; \
                 force the removal to discard them"
            ))
        } else {
            error.into()
        }
    };
    let removed = match &herdr_workspace_id {
        Some(workspace_id) => match herdr.remove_worktree(&machine, workspace_id, force) {
            Ok(()) => true,
            Err(error) if has_code(&error, WORKSPACE_NOT_FOUND) => false,
            Err(error) => return Err(dirty(error)),
        },
        None => false,
    };
    // herdr no longer has the workspace, e.g. the user closed it or herdr lost its session,
    // but the checkout may still exist: open it again to remove it.
    if !removed {
        let repository = match &location {
            WorkspaceLocation::Local { path } => path.as_str(),
            WorkspaceLocation::Ssh { path, .. } => path.as_str(),
        };
        match herdr.open_worktree(&machine, repository, &path) {
            Ok(workspace_id) => herdr
                .remove_worktree(&machine, &workspace_id, force)
                .map_err(dirty)?,
            // The checkout is already gone.
            Err(error) if has_code(&error, WORKTREE_NOT_FOUND) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(lock(storage).clear_task_worktree(id)?)
}

/// herdr API error codes that commands handle.
const DIRTY_WORKTREE: &str = "dirty_worktree_requires_force";
pub(super) const WORKSPACE_NOT_FOUND: &str = "workspace_not_found";
const WORKTREE_NOT_FOUND: &str = "worktree_not_found";

pub(super) fn has_code(error: &HerdrError, expected: &str) -> bool {
    matches!(error, HerdrError::Failed { code: Some(code), .. } if code == expected)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::adapters::git::fake::{FakeRunner, failed};
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

    /// Git for a repository whose base branch already exists.
    fn existing_base() -> Git {
        Git::new(Box::new(Arc::new(FakeRunner::new(vec![Ok(
            "abc\n".to_owned()
        )]))))
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

    /// The fixture task, started in `w27` and then stripped of its worktree as when it is
    /// removed in herdr.
    fn lost_worktree() -> Fixture {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, _) = herdr(vec![Ok(CREATED.to_owned())]);
        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();
        lock(&fixture.storage)
            .clear_task_worktree(fixture.task.id)
            .unwrap();
        fixture
    }

    const RECREATED: &str =
        r#"{"id":"cli:worktree:create","result":{"workspace":{"workspace_id":"w31"}}}"#;

    #[test]
    fn restore_checks_out_the_kept_branch_into_a_new_worktree() {
        let fixture = lost_worktree();
        let mut renamed = stored(&fixture);
        renamed.title = "Add sign-in".to_owned();
        lock(&fixture.storage).update_task(&renamed).unwrap();
        let (herdr, runner) = herdr(vec![Ok(RECREATED.to_owned())]);

        let restored =
            restore(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();

        assert_eq!(restored.status, TaskStatus::InProgress);
        assert_eq!(restored.branch.as_deref(), Some("feat/add-login-1"));
        assert_eq!(
            restored.worktree_path,
            Some(HostPath::Windows(
                WindowsPath::parse(r"D:\wt\toodo\feat-add-sign-in-1").unwrap()
            ))
        );
        assert_eq!(restored.herdr_workspace_id.as_deref(), Some("w31"));
        assert_eq!(stored(&fixture), restored);
        assert_eq!(
            invocations(&runner)[0].args[4..],
            [
                "--path",
                r"D:\wt\toodo\feat-add-sign-in-1",
                "--branch",
                "feat/add-login-1",
                "--base",
                "feat/add-login-1",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn restore_takes_a_branch_the_repository_lost_from_origin() {
        let fixture = lost_worktree();
        let git = Git::new(Box::new(Arc::new(FakeRunner::new(vec![
            Err(failed()),
            Ok("abc\n".to_owned()),
        ]))));
        let (herdr, runner) = herdr(vec![Ok(RECREATED.to_owned())]);

        restore(&fixture.storage, &git, &herdr, fixture.task.id).unwrap();

        let args = &invocations(&runner)[0].args;
        assert_eq!(args[args.len() - 2], "origin/feat/add-login-1");
    }

    #[test]
    fn restore_is_rejected_without_a_lost_worktree_or_a_branch() {
        let gone = || {
            Git::new(Box::new(Arc::new(FakeRunner::new(vec![
                Err(failed()),
                Err(failed()),
            ]))))
        };
        let (herdr, runner) = herdr(Vec::new());

        let ready = local_fixture(Some(r"D:\wt"));
        let started = local_fixture(Some(r"D:\wt"));
        let (start_herdr, _) = self::herdr(vec![Ok(CREATED.to_owned())]);
        start(
            &started.storage,
            &existing_base(),
            &start_herdr,
            started.task.id,
        )
        .unwrap();
        let lost = lost_worktree();
        for (fixture, git) in [(&ready, gone()), (&started, gone()), (&lost, gone())] {
            let error = restore(&fixture.storage, &git, &herdr, fixture.task.id).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::Rejected, "{}", error.message);
        }
        assert!(invocations(&runner).is_empty());
        assert_eq!(stored(&lost).worktree_path, None);
    }

    #[test]
    fn a_ready_task_starts_in_a_new_herdr_worktree() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);

        let started = start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();

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
    fn an_empty_repository_gets_an_initial_commit_before_the_worktree() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let git_runner = Arc::new(FakeRunner::new(vec![
            Err(failed()),
            Err(failed()),
            Ok("origin\n".to_owned()),
            Ok(String::new()),
            Ok(String::new()),
            Ok(String::new()),
        ]));
        let git = Git::new(Box::new(git_runner.clone()));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);

        let started = start(&fixture.storage, &git, &herdr, fixture.task.id).unwrap();

        assert_eq!(started.status, TaskStatus::InProgress);
        let git_commands: Vec<String> = invocations(&git_runner)
            .iter()
            .map(|invocation| invocation.args[2..].join(" "))
            .collect();
        assert_eq!(
            git_commands[4],
            "commit --allow-empty --only -m Initial commit"
        );
        assert_eq!(git_commands[5], "push --set-upstream origin main");
        assert_eq!(invocations(&runner)[0].args[..2], ["worktree", "create"]);
    }

    #[test]
    fn a_failed_initial_commit_leaves_the_task_ready_without_running_herdr() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let git = Git::new(Box::new(Arc::new(FakeRunner::new(vec![
            Err(failed()),
            Err(failed()),
            Ok(String::new()),
            Ok(String::new()),
            Err(failed()),
        ]))));
        let (herdr, runner) = herdr(Vec::new());

        let error = start(&fixture.storage, &git, &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::External);
        assert_eq!(stored(&fixture), fixture.task);
        assert!(invocations(&runner).is_empty());
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

        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();

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

        let error = start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        assert_eq!(error.message, "worktree root is not set for machine local");
        assert!(invocations(&runner).is_empty());
    }

    #[test]
    fn a_task_that_is_not_ready_is_rejected_before_herdr_runs() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);
        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();

        let error = start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap_err();

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

        let error = start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::External);
        assert!(error.message.ends_with("fatal: invalid reference: main"));
        assert_eq!(stored(&fixture), fixture.task);
    }

    const REMOVED: &str = r#"{"id":"cli:worktree:remove","result":{"type":"worktree_removed"}}"#;

    fn herdr_failed(code: &str) -> Result<String, GitError> {
        Err(GitError::Failed {
            command: "herdr worktree".to_owned(),
            stderr: format!(r#"{{"error":{{"code":"{code}","message":"{code}"}}}}"#),
        })
    }

    /// A local task started in herdr workspace `w27` and then cancelled; `results` follow
    /// the scripted `herdr worktree create`.
    fn cancelled_task(results: Vec<Result<String, GitError>>) -> (Fixture, Herdr, Arc<FakeRunner>) {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(
            [Ok(CREATED.to_owned())]
                .into_iter()
                .chain(results)
                .collect(),
        );
        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();
        lock(&fixture.storage)
            .set_task_status(fixture.task.id, TaskStatus::Cancelled)
            .unwrap();
        (fixture, herdr, runner)
    }

    #[test]
    fn a_finished_task_removes_its_worktree_and_keeps_its_branch() {
        let (fixture, herdr, runner) = cancelled_task(vec![Ok(REMOVED.to_owned())]);

        let task = remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap();

        assert_eq!(
            (&task.worktree_path, &task.herdr_workspace_id),
            (&None, &None)
        );
        assert_eq!(task.branch.as_deref(), Some("feat/add-login-1"));
        assert_eq!(task.status, TaskStatus::Cancelled);
        assert_eq!(stored(&fixture), task);
        assert_eq!(
            invocations(&runner)[1].args,
            ["worktree", "remove", "--workspace", "w27"].map(str::to_owned)
        );
    }

    #[test]
    fn a_dirty_worktree_is_kept_unless_forced() {
        let (fixture, herdr, runner) = cancelled_task(vec![
            herdr_failed("dirty_worktree_requires_force"),
            Ok(REMOVED.to_owned()),
        ]);
        let before = stored(&fixture);

        let error = remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains(r"D:\wt\toodo\feat-add-login-1"));
        assert_eq!(stored(&fixture), before);

        remove_worktree(&fixture.storage, &herdr, fixture.task.id, true).unwrap();
        assert_eq!(
            invocations(&runner)[2].args,
            ["worktree", "remove", "--workspace", "w27", "--force"].map(str::to_owned)
        );
        assert_eq!(stored(&fixture).worktree_path, None);
    }

    #[test]
    fn a_worktree_herdr_no_longer_has_open_is_opened_again_to_remove_it() {
        let opened = r#"{"id":"cli:worktree:open","result":{"workspace":{"workspace_id":"w40"}}}"#;
        let (fixture, herdr, runner) = cancelled_task(vec![
            herdr_failed("workspace_not_found"),
            Ok(opened.to_owned()),
            Ok(REMOVED.to_owned()),
        ]);

        remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap();

        let invocations = invocations(&runner);
        assert_eq!(
            invocations[2].args,
            [
                "worktree",
                "open",
                "--cwd",
                r"C:\dev\toodo",
                "--path",
                r"D:\wt\toodo\feat-add-login-1",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            invocations[3].args,
            ["worktree", "remove", "--workspace", "w40"].map(str::to_owned)
        );
        assert_eq!(stored(&fixture).worktree_path, None);
    }

    #[test]
    fn a_worktree_that_is_already_gone_is_only_forgotten() {
        let (fixture, herdr, runner) = cancelled_task(vec![
            herdr_failed("workspace_not_found"),
            herdr_failed("worktree_not_found"),
        ]);

        let task = remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap();

        assert_eq!(
            (&task.worktree_path, &task.herdr_workspace_id),
            (&None, &None)
        );
        assert_eq!(invocations(&runner).len(), 3);
    }

    #[test]
    fn other_herdr_failures_keep_the_worktree() {
        let (fixture, herdr, _) = cancelled_task(vec![herdr_failed("worktree_remove_failed")]);
        let before = stored(&fixture);

        let error = remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::External);
        assert_eq!(stored(&fixture), before);
    }

    #[test]
    fn only_finished_tasks_with_a_worktree_remove_it() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);
        let id = fixture.task.id;

        lock(&fixture.storage)
            .set_task_status(id, TaskStatus::Cancelled)
            .unwrap();
        let error = remove_worktree(&fixture.storage, &herdr, id, false).unwrap_err();
        assert_eq!(error.message, format!("task {id} has no worktree"));

        let fixture = local_fixture(Some(r"D:\wt"));
        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();
        let error = remove_worktree(&fixture.storage, &herdr, fixture.task.id, false).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("only done or cancelled tasks"));
        assert_eq!(invocations(&runner).len(), 1);
    }

    #[test]
    fn deleting_a_task_removes_its_worktree_first() {
        let (fixture, herdr, runner) = cancelled_task(vec![Ok(REMOVED.to_owned())]);

        delete(&fixture.storage, &herdr, fixture.task.id, false).unwrap();

        assert_eq!(
            invocations(&runner)[1].args,
            ["worktree", "remove", "--workspace", "w27"].map(str::to_owned)
        );
        assert_eq!(
            lock(&fixture.storage).get_task(fixture.task.id).unwrap(),
            None
        );
    }

    #[test]
    fn a_dirty_worktree_keeps_the_task_unless_the_delete_is_forced() {
        let (fixture, herdr, runner) = cancelled_task(vec![
            herdr_failed("dirty_worktree_requires_force"),
            Ok(REMOVED.to_owned()),
        ]);
        let before = stored(&fixture);

        let error = delete(&fixture.storage, &herdr, fixture.task.id, false).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert_eq!(stored(&fixture), before);

        delete(&fixture.storage, &herdr, fixture.task.id, true).unwrap();
        assert_eq!(
            invocations(&runner)[2].args,
            ["worktree", "remove", "--workspace", "w27", "--force"].map(str::to_owned)
        );
        assert_eq!(
            lock(&fixture.storage).get_task(fixture.task.id).unwrap(),
            None
        );
    }

    #[test]
    fn deleting_a_task_without_a_worktree_does_not_run_herdr() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(Vec::new());

        delete(&fixture.storage, &herdr, fixture.task.id, false).unwrap();

        assert!(invocations(&runner).is_empty());
        assert_eq!(
            lock(&fixture.storage).get_task(fixture.task.id).unwrap(),
            None
        );
    }

    #[test]
    fn open_focuses_the_herdr_workspace_of_a_started_task() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned()), Ok(String::new())]);

        let error = open(&fixture.storage, &herdr, fixture.task.id).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);

        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();
        open(&fixture.storage, &herdr, fixture.task.id).unwrap();
        assert_eq!(
            invocations(&runner)[1].args,
            ["workspace", "focus", "w27"].map(str::to_owned)
        );
    }

    #[test]
    fn open_reopens_a_worktree_whose_herdr_workspace_is_gone() {
        let fixture = local_fixture(Some(r"D:\wt"));
        let opened = r#"{"id":"cli:worktree:open","result":{"workspace":{"workspace_id":"w40"}}}"#;
        let (herdr, runner) = herdr(vec![
            Ok(CREATED.to_owned()),
            herdr_failed("workspace_not_found"),
            Ok(opened.to_owned()),
            Ok(String::new()),
        ]);
        start(&fixture.storage, &existing_base(), &herdr, fixture.task.id).unwrap();

        open(&fixture.storage, &herdr, fixture.task.id).unwrap();

        let invocations = invocations(&runner);
        assert_eq!(invocations[2].args[..2], ["worktree", "open"]);
        assert_eq!(
            invocations[3].args,
            ["workspace", "focus", "w40"].map(str::to_owned)
        );
        assert_eq!(stored(&fixture).herdr_workspace_id.as_deref(), Some("w40"));
    }
}
