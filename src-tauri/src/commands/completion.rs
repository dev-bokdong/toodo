//! Commands that finish a task (see `docs/workflow.md`): recording the commits of its
//! branch, opening its pull request, which pushes the branch, opens (or reuses) a pull
//! request with `gh`, and moves the task to `in_review`, and completing it once that
//! pull request is merged, which closes its herdr workspace (keeping the worktree), moves
//! it to `done`, and releases its dependents.
//!
//! They run on the async runtime because git and gh can take seconds. The database lock
//! is not held while they run.
//!
//! A background thread also completes `in_review` tasks on its own: it checks their pull
//! requests every minute and completes each task whose pull request was merged, as the
//! `complete_task` command would, telling the frontend through [`TasksChanged`].
//!
//! [`TasksChanged`]: super::events::TasksChanged

use std::collections::HashMap;
use std::io;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};

use super::events::Notifier;
use super::herdr::{WORKSPACE_NOT_FOUND, has_code};
use super::types::{self, CommitRecord, Task, TaskId};
use super::{CommandError, CommandResult, GitHubState, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::adapters::github::{GitHub, NewPullRequest, PullRequest, PullRequestState};
use crate::adapters::herdr::Herdr;
use crate::domain::model::{self, WorkspaceLocation};
use crate::domain::remote::GitHubRepository;
use crate::domain::status::TaskStatus;
use crate::storage::{Storage, StorageError};

/// Reads the commits of an `in_progress` or `in_review` task's branch that are not on
/// the base branch and stores them as its commit records.
#[tauri::command(async)]
#[specta::specta]
pub fn refresh_commit_records(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    task_id: TaskId,
) -> CommandResult<Vec<CommitRecord>> {
    Ok(types::map_all(&refresh(&storage, &git, task_id.into())?))
}

/// Records the commits of an `in_progress` or `in_review` task, pushes its branch, opens
/// a pull request into the base branch, and moves the task to `in_review`. A pull request
/// of the branch opened elsewhere (e.g. from herdr) is adopted instead: an open one gets
/// the push, a merged one is recorded without pushing. Run it again after more commits
/// to push them to the pull request.
#[tauri::command(async)]
#[specta::specta]
pub fn open_pull_request(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    github: State<'_, GitHubState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&open(&storage, &git, &github, id.into())?).into())
}

/// Moves an `in_review` task to `done` once its pull request is merged, which may make
/// its dependents `ready`. Its herdr workspace is closed; the worktree stays on disk.
#[tauri::command(async)]
#[specta::specta]
pub fn complete_task(
    storage: State<'_, StorageState>,
    github: State<'_, GitHubState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&complete(&storage, &github, &herdr, id.into())?).into())
}

/// An `in_progress` or `in_review` task with the workspace and worktree its git commands
/// run in.
struct Checkout {
    task: model::Task,
    location: WorkspaceLocation,
    base: String,
    branch: String,
    worktree: String,
}

fn checkout(storage: &StorageState, id: model::TaskId) -> CommandResult<Checkout> {
    let storage = lock(storage);
    let task = storage
        .get_task(id)?
        .ok_or(StorageError::TaskNotFound(id))?;
    if !matches!(task.status, TaskStatus::InProgress | TaskStatus::InReview) {
        return Err(CommandError::rejected(format!(
            "task {id} is {}; only an in_progress or in_review task can record commits or open a pull request",
            task.status
        )));
    }
    let (Some(branch), Some(worktree)) = (task.branch.clone(), task.worktree_path.as_ref()) else {
        return Err(CommandError::rejected(format!(
            "task {id} has no worktree; start it in herdr first"
        )));
    };
    let worktree = worktree.to_string();
    let workspace = storage
        .get_workspace(task.workspace_id)?
        .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
    Ok(Checkout {
        task,
        location: workspace.location,
        base: workspace.default_base_branch,
        branch,
        worktree,
    })
}

fn commit_records(git: &Git, checkout: &Checkout) -> CommandResult<Vec<model::CommitRecord>> {
    let commits = git.commits_between(
        &checkout.location,
        &checkout.worktree,
        &checkout.base,
        &checkout.branch,
    )?;
    Ok(commits
        .into_iter()
        .map(|commit| model::CommitRecord {
            task_id: checkout.task.id,
            sha: commit.sha,
            message: commit.message,
            author: commit.author,
            committed_at: commit.committed_at,
        })
        .collect())
}

fn refresh(
    storage: &StorageState,
    git: &Git,
    id: model::TaskId,
) -> CommandResult<Vec<model::CommitRecord>> {
    let checkout = checkout(storage, id)?;
    let records = commit_records(git, &checkout)?;
    lock(storage).replace_commit_records(id, &records)?;
    Ok(records)
}

fn open(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    id: model::TaskId,
) -> CommandResult<model::Task> {
    let checkout = checkout(storage, id)?;
    let records = commit_records(git, &checkout)?;
    let origin = git.origin_url(&checkout.location, &checkout.worktree)?;
    let repository = GitHubRepository::from_remote_url(&origin).ok_or_else(|| {
        CommandError::rejected(format!(
            "remote origin {origin:?} is not a GitHub repository; pull requests are opened on GitHub only"
        ))
    })?;
    // A closed pull request is not reused: a new one is opened into the right base.
    let existing = github
        .find_pull_request(&repository, &checkout.branch)?
        .filter(|pull_request| pull_request.state != PullRequestState::Closed);
    if let Some(pull_request) = &existing {
        check_base(pull_request, &checkout.base)?;
    }
    if let Some(PullRequest {
        url,
        state: PullRequestState::Merged,
        merged_at,
        ..
    }) = existing
    {
        // After the merge the base may already contain the branch, so its log can be
        // empty; keep the commits recorded before in that case.
        let mut storage = lock(storage);
        let records = if records.is_empty() {
            storage.list_commit_records(id)?
        } else {
            records
        };
        return Ok(storage.open_pull_request(id, &records, &url, merged_at)?);
    }
    if records.is_empty() {
        return Err(CommandError::rejected(format!(
            "branch {} has no commits that are not on {}; commit the work before opening a pull request",
            checkout.branch, checkout.base
        )));
    }
    git.push_branch(&checkout.location, &checkout.worktree, &checkout.branch)?;
    let pull_request_url = match existing {
        Some(pull_request) => pull_request.url,
        None => github.create_pull_request(&NewPullRequest {
            repository: &repository,
            head: &checkout.branch,
            base: &checkout.base,
            title: &checkout.task.title,
            body: &checkout.task.description,
        })?,
    };
    Ok(lock(storage).open_pull_request(id, &records, &pull_request_url, None)?)
}

/// Rejects a pull request that does not merge into the workspace's base branch: the
/// worktrees of dependent tasks start from that branch, so they would miss the work.
fn check_base(pull_request: &PullRequest, base: &str) -> CommandResult<()> {
    if pull_request.base == base {
        return Ok(());
    }
    Err(CommandError::rejected(format!(
        "pull request {} merges into {}, not the workspace base branch {base}; dependent tasks start from {base}, so retarget it or open a new one",
        pull_request.url, pull_request.base
    )))
}

fn complete(
    storage: &StorageState,
    github: &GitHub,
    herdr: &Herdr,
    id: model::TaskId,
) -> CommandResult<model::Task> {
    match complete_if_merged(storage, github, herdr, id)? {
        Completion::Completed(task) => Ok(*task),
        Completion::Open(url) => Err(CommandError::rejected(format!(
            "pull request {url} is not merged yet; merge it before completing the task"
        ))),
        Completion::Closed(url) => Err(CommandError::rejected(format!(
            "pull request {url} was closed without merging; reopen it or pause the task"
        ))),
    }
}

/// What completing an `in_review` task found about its pull request.
enum Completion {
    /// The pull request was merged and the task is now `done`.
    Completed(Box<model::Task>),
    /// The pull request at this URL is still open; the task stays `in_review`.
    Open(String),
    /// The pull request at this URL was closed without merging; the task stays
    /// `in_review`.
    Closed(String),
}

/// Completes an `in_review` task if its pull request was merged.
fn complete_if_merged(
    storage: &StorageState,
    github: &GitHub,
    herdr: &Herdr,
    id: model::TaskId,
) -> CommandResult<Completion> {
    let (task, location, base) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        (task, workspace.location, workspace.default_base_branch)
    };
    let pull_request_url = match (task.status, task.pull_request_url) {
        (TaskStatus::InReview, Some(url)) => url,
        (status, _) => {
            return Err(CommandError::rejected(format!(
                "task {id} is {status} without a pull request; only an in_review task can be completed"
            )));
        }
    };
    let pull_request = github.pull_request(&pull_request_url)?;
    check_base(&pull_request, &base)?;
    match pull_request.state {
        PullRequestState::Merged => {}
        PullRequestState::Open => return Ok(Completion::Open(pull_request_url)),
        PullRequestState::Closed => return Ok(Completion::Closed(pull_request_url)),
    }
    // A workspace herdr no longer has was already closed, e.g. by the user.
    if let Some(workspace_id) = &task.herdr_workspace_id {
        match herdr.close_workspace(&location.machine(), workspace_id) {
            Ok(()) => {}
            Err(error) if has_code(&error, WORKSPACE_NOT_FOUND) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Completion::Completed(Box::new(
        lock(storage).complete_task(id, pull_request.merged_at)?,
    )))
}

/// How often the pull requests of `in_review` tasks are checked for a merge.
const MERGE_CHECK_PERIOD: Duration = Duration::from_secs(60);

/// Starts the background thread that completes `in_review` tasks whose pull request was
/// merged.
pub fn spawn(app: AppHandle) -> io::Result<()> {
    thread::Builder::new()
        .name("pull-request-merges".to_owned())
        .spawn(move || {
            let mut failures = HashMap::new();
            loop {
                complete_merged(
                    &app.state::<StorageState>(),
                    &app.state::<GitHubState>(),
                    &app.state::<HerdrState>(),
                    &app,
                    &mut failures,
                );
                thread::sleep(MERGE_CHECK_PERIOD);
            }
        })?;
    Ok(())
}

/// Completes every `in_review` task whose pull request was merged and announces each.
/// `failures` holds the last failure of each task, so a failure that repeats on every
/// check (e.g. `gh` is not signed in) is logged once.
fn complete_merged(
    storage: &StorageState,
    github: &GitHub,
    herdr: &Herdr,
    notifier: &dyn Notifier,
    failures: &mut HashMap<model::TaskId, String>,
) {
    let tasks = match in_review_tasks(&lock(storage)) {
        Ok(tasks) => tasks,
        Err(error) => {
            eprintln!("toodo: failed to list tasks in review: {error}");
            return;
        }
    };
    failures.retain(|id, _| tasks.contains(id));
    for id in tasks {
        match complete_if_merged(storage, github, herdr, id) {
            Ok(completion) => {
                failures.remove(&id);
                if let Completion::Completed(task) = completion {
                    notifier.tasks_changed(task.workspace_id);
                }
            }
            Err(error) => {
                if failures.get(&id) != Some(&error.message) {
                    eprintln!(
                        "toodo: failed to complete task {id} with a merged pull request: {}",
                        error.message
                    );
                    failures.insert(id, error.message);
                }
            }
        }
    }
}

/// IDs of the `in_review` tasks of all workspaces that have a pull request.
fn in_review_tasks(storage: &Storage) -> Result<Vec<model::TaskId>, StorageError> {
    let mut ids = Vec::new();
    for workspace in storage.list_workspaces()? {
        ids.extend(
            storage
                .list_tasks(workspace.id)?
                .into_iter()
                .filter(|task| {
                    task.status == TaskStatus::InReview && task.pull_request_url.is_some()
                })
                .map(|task| task.id),
        );
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::adapters::git::fake::{FakeRunner, failed};
    use crate::adapters::git::{GitError, Invocation};
    use crate::commands::CommandErrorKind;
    use crate::domain::model::{HostPath, MachineLabel, TaskType};
    use crate::domain::naming::TaskCheckout;
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::storage::{NewTask, NewWorkspace, Storage};

    const SHA_A: &str = "1111111111111111111111111111111111111111";
    const SHA_B: &str = "2222222222222222222222222222222222222222";
    const PR: &str = "https://github.com/acme/toodo/pull/12";

    struct Fixture {
        storage: StorageState,
        task: model::Task,
        dependent: model::Task,
    }

    fn fixture(location: WorkspaceLocation, worktree: HostPath) -> Fixture {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "toodo".to_owned(),
                location,
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let new_task = |title: &str| NewTask {
            workspace_id: workspace.id,
            task_type: TaskType::Feat,
            title: title.to_owned(),
            description: format!("Implement {title}"),
        };
        let task = storage.create_task(&new_task("Add login")).unwrap();
        let dependent = storage.create_task(&new_task("Use login")).unwrap();
        storage.add_dependency(dependent.id, task.id).unwrap();
        let checkout = TaskCheckout {
            path: worktree,
            branch: "feat/add-login-1".to_owned(),
        };
        let task = storage.start_task(task.id, &checkout, "w27").unwrap();
        Fixture {
            storage: Mutex::new(storage),
            task,
            dependent,
        }
    }

    fn local_fixture() -> Fixture {
        fixture(
            WorkspaceLocation::Local {
                path: WindowsPath::parse(r"C:\dev\toodo").unwrap(),
            },
            HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-add-login-1").unwrap()),
        )
    }

    fn runner(results: Vec<Result<String, GitError>>) -> Arc<FakeRunner> {
        Arc::new(FakeRunner::new(results))
    }

    fn adapters(git: &Arc<FakeRunner>, gh: &Arc<FakeRunner>) -> (Git, GitHub) {
        (
            Git::new(Box::new(git.clone())),
            GitHub::new(Box::new(gh.clone())),
        )
    }

    fn log(shas: &[&str]) -> Result<String, GitError> {
        Ok(shas
            .iter()
            .enumerate()
            .map(|(index, sha)| {
                format!(
                    "{sha}\u{1f}Ada <ada@example.com>\u{1f}{index}\u{1f}Commit {index}\n\u{1e}\n"
                )
            })
            .collect())
    }

    fn origin(url: &str) -> Result<String, GitError> {
        Ok(format!("{url}\n"))
    }

    fn invocations(runner: &FakeRunner) -> Vec<Invocation> {
        runner.invocations.lock().unwrap().clone()
    }

    fn git_commands(runner: &FakeRunner) -> Vec<String> {
        invocations(runner)
            .iter()
            .map(|invocation| invocation.args[2].clone())
            .collect()
    }

    /// A herdr that must not be called.
    fn no_herdr() -> Herdr {
        Herdr::new(Box::new(runner(Vec::new())))
    }

    /// A herdr that closes one workspace.
    fn closing_herdr() -> (Herdr, Arc<FakeRunner>) {
        let runner = runner(vec![Ok(
            r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#.to_owned(),
        )]);
        (Herdr::new(Box::new(runner.clone())), runner)
    }

    fn dependent_ready(fixture: &Fixture) -> bool {
        let tasks = lock(&fixture.storage).list_all_tasks().unwrap();
        crate::domain::dag::is_ready(&tasks, fixture.dependent.id).unwrap()
    }

    fn stored(fixture: &Fixture, task: &model::Task) -> model::Task {
        lock(&fixture.storage).get_task(task.id).unwrap().unwrap()
    }

    #[test]
    fn opening_a_pull_request_records_commits_pushes_and_puts_the_task_in_review() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            log(&[SHA_A, SHA_B]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
        ]);
        let gh_runner = runner(vec![Ok("[]".to_owned()), Ok(format!("{PR}\n"))]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        assert_eq!(in_review.status, TaskStatus::InReview);
        assert_eq!(in_review.pull_request_url.as_deref(), Some(PR));
        assert_eq!(stored(&fixture, &fixture.task), in_review);
        let records = lock(&fixture.storage)
            .list_commit_records(fixture.task.id)
            .unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.sha.as_str())
                .collect::<Vec<_>>(),
            [SHA_A, SHA_B]
        );
        assert!(!dependent_ready(&fixture));
        assert_eq!(git_commands(&git_runner), ["log", "remote", "push"]);
        assert_eq!(
            invocations(&git_runner)[0].args[..2],
            ["-C", r"D:\wt\toodo\feat-add-login-1"].map(str::to_owned)
        );
        assert_eq!(
            invocations(&gh_runner)[1].args,
            [
                "pr",
                "create",
                "--repo",
                "github.com/acme/toodo",
                "--head",
                "feat/add-login-1",
                "--base",
                "main",
                "--title",
                "Add login",
                "--body",
                "Implement Add login",
            ]
            .map(str::to_owned)
        );
    }

    #[test]
    fn ssh_tasks_run_git_on_their_machine_and_gh_locally() {
        let devbox = MachineLabel::parse("devbox").unwrap();
        let fixture = fixture(
            WorkspaceLocation::Ssh {
                machine: devbox,
                path: PosixPath::parse("/srv/toodo").unwrap(),
            },
            HostPath::Posix(PosixPath::parse("/wt/toodo/feat-add-login-1").unwrap()),
        );
        let git_runner = runner(vec![
            log(&[SHA_A]),
            origin("https://github.com/acme/toodo.git"),
            Ok(String::new()),
        ]);
        let gh_runner = runner(vec![Ok("[]".to_owned()), Ok(format!("{PR}\n"))]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        let push = &invocations(&git_runner)[2];
        assert_eq!(push.program, "ssh");
        assert_eq!(
            push.args[4],
            "git '-C' '/wt/toodo/feat-add-login-1' 'push' '--set-upstream' 'origin' 'feat/add-login-1'"
        );
        assert_eq!(invocations(&gh_runner)[1].program, "gh");
    }

    #[test]
    fn a_branch_without_commits_is_rejected_before_anything_is_pushed() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[]), origin("git@github.com:acme/toodo.git")]);
        let gh_runner = runner(vec![Ok("[]".to_owned())]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error
                .message
                .contains("has no commits that are not on main")
        );
        assert_eq!(git_commands(&git_runner), ["log", "remote"]);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(stored(&fixture, &fixture.task), fixture.task);
    }

    #[test]
    fn a_non_github_origin_is_rejected_before_anything_is_pushed() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[SHA_A]), origin("/srv/git/toodo.git")]);
        let gh_runner = runner(Vec::new());
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("is not a GitHub repository"));
        assert_eq!(git_commands(&git_runner), ["log", "remote"]);
        assert!(invocations(&gh_runner).is_empty());
    }

    #[test]
    fn a_failed_pull_request_keeps_the_task_in_progress_and_a_retry_reuses_the_open_one() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            log(&[SHA_A]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
            log(&[SHA_A, SHA_B]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
        ]);
        let gh_runner = runner(vec![
            Ok("[]".to_owned()),
            Err(failed()),
            listed("OPEN", "main"),
        ]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, fixture.task.id).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::External);
        assert_eq!(stored(&fixture, &fixture.task), fixture.task);
        assert_eq!(
            lock(&fixture.storage)
                .list_commit_records(fixture.task.id)
                .unwrap(),
            []
        );

        let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();
        assert_eq!(in_review.pull_request_url.as_deref(), Some(PR));
        assert_eq!(invocations(&gh_runner).len(), 3);
        assert_eq!(invocations(&gh_runner)[2].args[1], "list");
    }

    #[test]
    fn refresh_stores_the_commits_of_an_in_progress_task_only() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[SHA_A]), log(&[SHA_A, SHA_B])]);
        let git = Git::new(Box::new(git_runner.clone()));

        assert_eq!(
            refresh(&fixture.storage, &git, fixture.task.id)
                .unwrap()
                .len(),
            1
        );
        let records = refresh(&fixture.storage, &git, fixture.task.id).unwrap();
        assert_eq!(
            lock(&fixture.storage)
                .list_commit_records(fixture.task.id)
                .unwrap(),
            records
        );
        assert_eq!(records.len(), 2);

        let error = refresh(&fixture.storage, &git, fixture.dependent.id).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        let error = refresh(&fixture.storage, &git, model::TaskId(999)).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(invocations(&git_runner).len(), 2);
    }

    #[test]
    fn an_in_progress_task_without_a_worktree_is_rejected() {
        let fixture = local_fixture();
        for status in [TaskStatus::InReview, TaskStatus::Done] {
            lock(&fixture.storage)
                .set_task_status(fixture.task.id, status)
                .unwrap();
        }
        let ready = fixture.dependent.id;
        lock(&fixture.storage)
            .set_task_status(ready, TaskStatus::InProgress)
            .unwrap();
        let git_runner = runner(Vec::new());
        let gh_runner = runner(Vec::new());
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, ready).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("has no worktree"));
        assert!(invocations(&git_runner).is_empty());
    }

    fn in_review_fixture() -> Fixture {
        let fixture = local_fixture();
        lock(&fixture.storage)
            .open_pull_request(fixture.task.id, &[], PR, None)
            .unwrap();
        fixture
    }

    /// `mergedAt` of a merged pull request in the `gh` output, and the same time in
    /// seconds since the Unix epoch.
    const MERGED_AT: (&str, i64) = ("2026-10-09T14:47:45Z", 1_791_557_265);

    fn pull_request_json(state: &str, base: &str) -> String {
        let merged_at = if state == "MERGED" {
            format!("\"{}\"", MERGED_AT.0)
        } else {
            "null".to_owned()
        };
        format!(
            r#"{{"baseRefName":"{base}","mergedAt":{merged_at},"state":"{state}","url":"{PR}"}}"#
        )
    }

    /// `gh pr list` output with one pull request.
    fn listed(state: &str, base: &str) -> Result<String, GitError> {
        Ok(format!("[{}]", pull_request_json(state, base)))
    }

    /// `gh pr view` output.
    fn viewed(state: &str, base: &str) -> Result<String, GitError> {
        Ok(pull_request_json(state, base))
    }

    fn recorded_shas(fixture: &Fixture) -> Vec<String> {
        lock(&fixture.storage)
            .list_commit_records(fixture.task.id)
            .unwrap()
            .into_iter()
            .map(|record| record.sha)
            .collect()
    }

    #[test]
    fn an_open_pull_request_opened_outside_toodo_is_adopted_and_pushed() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            log(&[SHA_A]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
        ]);
        let gh_runner = runner(vec![listed("OPEN", "main")]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        assert_eq!(in_review.status, TaskStatus::InReview);
        assert_eq!(in_review.pull_request_url.as_deref(), Some(PR));
        assert_eq!(git_commands(&git_runner), ["log", "remote", "push"]);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(recorded_shas(&fixture), [SHA_A]);
    }

    #[test]
    fn a_merged_pull_request_is_adopted_without_pushing_and_can_be_completed() {
        let fixture = local_fixture();
        lock(&fixture.storage)
            .replace_commit_records(
                fixture.task.id,
                &[model::CommitRecord {
                    task_id: fixture.task.id,
                    sha: SHA_A.to_owned(),
                    message: "Add login".to_owned(),
                    author: "Ada <ada@example.com>".to_owned(),
                    committed_at: 1,
                }],
            )
            .unwrap();
        let git_runner = runner(vec![log(&[]), origin("git@github.com:acme/toodo.git")]);
        let gh_runner = runner(vec![listed("MERGED", "main"), viewed("MERGED", "main")]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        assert_eq!(in_review.status, TaskStatus::InReview);
        assert_eq!(in_review.pull_request_url.as_deref(), Some(PR));
        assert_eq!(in_review.pull_request_merged_at, Some(MERGED_AT.1));
        assert_eq!(git_commands(&git_runner), ["log", "remote"]);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(recorded_shas(&fixture), [SHA_A]);
        assert!(!dependent_ready(&fixture));

        let completed = complete(
            &fixture.storage,
            &github,
            &closing_herdr().0,
            fixture.task.id,
        )
        .unwrap();
        assert_eq!(completed.status, TaskStatus::Done);
        assert!(dependent_ready(&fixture));
    }

    #[test]
    fn a_closed_pull_request_is_replaced_by_a_new_one() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            log(&[SHA_A]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
        ]);
        let new_pr = "https://github.com/acme/toodo/pull/13";
        let gh_runner = runner(vec![listed("CLOSED", "develop"), Ok(format!("{new_pr}\n"))]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        assert_eq!(in_review.pull_request_url.as_deref(), Some(new_pr));
        assert_eq!(git_commands(&git_runner), ["log", "remote", "push"]);
        assert_eq!(invocations(&gh_runner)[1].args[..2], ["pr", "create"]);
    }

    #[test]
    fn a_pull_request_into_another_base_is_rejected_when_opening() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[SHA_A]), origin("git@github.com:acme/toodo.git")]);
        let gh_runner = runner(vec![listed("OPEN", "develop")]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error
                .message
                .contains("merges into develop, not the workspace base branch main"),
            "{}",
            error.message
        );
        assert_eq!(git_commands(&git_runner), ["log", "remote"]);
        assert_eq!(stored(&fixture, &fixture.task), fixture.task);
        assert!(recorded_shas(&fixture).is_empty());
    }

    #[test]
    fn a_pull_request_into_another_base_is_rejected_when_completing() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![viewed("MERGED", "develop")])));

        let error = complete(&fixture.storage, &github, &no_herdr(), fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("merges into develop"));
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert!(!dependent_ready(&fixture));
    }

    #[test]
    fn completing_a_task_with_a_merged_pull_request_finishes_it_and_releases_dependents() {
        let fixture = in_review_fixture();
        let gh_runner = runner(vec![viewed("MERGED", "main")]);
        let github = GitHub::new(Box::new(gh_runner.clone()));
        let (herdr, herdr_runner) = closing_herdr();

        let completed = complete(&fixture.storage, &github, &herdr, fixture.task.id).unwrap();

        assert_eq!(completed.status, TaskStatus::Done);
        assert_eq!(completed.pull_request_url.as_deref(), Some(PR));
        assert_eq!(completed.pull_request_merged_at, Some(MERGED_AT.1));
        assert_eq!(
            completed.timeline.last().map(|event| event.status),
            Some(TaskStatus::Done)
        );
        assert_eq!(completed.herdr_workspace_id, None);
        assert_eq!(completed.worktree_path, fixture.task.worktree_path);
        assert_eq!(stored(&fixture, &fixture.task), completed);
        assert_eq!(
            invocations(&herdr_runner)[0].args,
            ["workspace", "close", "w27"].map(str::to_owned)
        );
        assert!(dependent_ready(&fixture));
        assert_eq!(
            invocations(&gh_runner)[0].args,
            ["pr", "view", PR, "--json", "url,state,baseRefName,mergedAt"].map(str::to_owned)
        );
    }

    #[test]
    fn a_pull_request_that_is_not_merged_keeps_the_task_in_review() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![
            viewed("OPEN", "main"),
            viewed("CLOSED", "main"),
        ])));

        for expected in ["is not merged yet", "was closed without merging"] {
            let error =
                complete(&fixture.storage, &github, &no_herdr(), fixture.task.id).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::Rejected);
            assert!(error.message.contains(expected), "{}", error.message);
        }
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert!(!dependent_ready(&fixture));
    }

    #[test]
    fn only_an_in_review_task_can_be_completed() {
        let fixture = local_fixture();
        let gh_runner = runner(Vec::new());
        let github = GitHub::new(Box::new(gh_runner.clone()));

        let error = complete(&fixture.storage, &github, &no_herdr(), fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("only an in_review task"));
        assert!(invocations(&gh_runner).is_empty());
        let error =
            complete(&fixture.storage, &github, &no_herdr(), model::TaskId(999)).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
    }

    #[test]
    fn a_herdr_workspace_that_is_already_closed_does_not_block_completion() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![viewed("MERGED", "main")])));
        let herdr = Herdr::new(Box::new(runner(vec![Err(GitError::Failed {
            command: "herdr workspace close".to_owned(),
            stderr:
                r#"{"error":{"code":"workspace_not_found","message":"workspace w27 not found"}}"#
                    .to_owned(),
        })])));

        let completed = complete(&fixture.storage, &github, &herdr, fixture.task.id).unwrap();

        assert_eq!(completed.status, TaskStatus::Done);
        assert_eq!(completed.herdr_workspace_id, None);
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<model::WorkspaceId>>);

    impl Notifier for Recorder {
        fn tasks_changed(&self, workspace: model::WorkspaceId) {
            self.0.lock().unwrap().push(workspace);
        }

        fn agent_statuses_changed(&self, _: &[types::TaskAgentStatus]) {}
    }

    #[test]
    fn a_merged_pull_request_completes_its_task_automatically() {
        let fixture = in_review_fixture();
        let gh_runner = runner(vec![viewed("OPEN", "main"), viewed("MERGED", "main")]);
        let github = GitHub::new(Box::new(gh_runner.clone()));
        let (herdr, herdr_runner) = closing_herdr();
        let recorder = Recorder::default();
        let mut failures = HashMap::new();

        complete_merged(&fixture.storage, &github, &herdr, &recorder, &mut failures);
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert!(recorder.0.lock().unwrap().is_empty());
        assert!(invocations(&herdr_runner).is_empty());

        complete_merged(&fixture.storage, &github, &herdr, &recorder, &mut failures);
        let done = stored(&fixture, &fixture.task);
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(done.pull_request_merged_at, Some(MERGED_AT.1));
        assert!(dependent_ready(&fixture));
        assert_eq!(*recorder.0.lock().unwrap(), [done.workspace_id]);
        assert_eq!(invocations(&herdr_runner).len(), 1);
        assert!(failures.is_empty());

        // A done task is no longer checked.
        complete_merged(&fixture.storage, &github, &herdr, &recorder, &mut failures);
        assert_eq!(invocations(&gh_runner).len(), 2);
    }

    #[test]
    fn a_failed_check_keeps_the_task_in_review_and_remembers_the_failure() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![
            Err(failed()),
            viewed("OPEN", "main"),
        ])));
        let recorder = Recorder::default();
        let mut failures = HashMap::new();

        complete_merged(
            &fixture.storage,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
        );
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert_eq!(failures.keys().collect::<Vec<_>>(), [&fixture.task.id]);

        complete_merged(
            &fixture.storage,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
        );
        assert!(failures.is_empty());
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_herdr_failure_keeps_the_task_in_review() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![viewed("MERGED", "main")])));
        let herdr = Herdr::new(Box::new(runner(vec![Err(GitError::Spawn {
            program: "herdr".to_owned(),
            message: "program not found".to_owned(),
        })])));
        let before = stored(&fixture, &fixture.task);

        let error = complete(&fixture.storage, &github, &herdr, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::External);
        assert_eq!(stored(&fixture, &fixture.task), before);
        assert!(!dependent_ready(&fixture));
    }
}
