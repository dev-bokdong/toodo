//! Commands that finish a task (see `docs/workflow.md`): recording the commits of its
//! branch, opening its pull request, which pushes the branch, opens (or reuses) a pull
//! request with `gh`, and moves the task to `in_review`, and completing it once that
//! pull request is merged, which moves it to `done` and releases its dependents.
//!
//! They run on the async runtime because git and gh can take seconds. The database lock
//! is not held while they run.

use tauri::State;

use super::types::{self, CommitRecord, Task, TaskId};
use super::{CommandError, CommandResult, GitHubState, GitState, StorageState, lock};
use crate::adapters::git::Git;
use crate::adapters::github::{GitHub, NewPullRequest, PullRequestState};
use crate::domain::model::{self, WorkspaceLocation};
use crate::domain::remote::GitHubRepository;
use crate::domain::status::TaskStatus;
use crate::storage::StorageError;

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
/// a pull request into the base branch (or reuses the open one), and moves the task to
/// `in_review`. Run it again after more commits to push them to the pull request.
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
/// its dependents `ready`.
#[tauri::command(async)]
#[specta::specta]
pub fn complete_task(
    storage: State<'_, StorageState>,
    github: State<'_, GitHubState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&complete(&storage, &github, id.into())?).into())
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
    if records.is_empty() {
        return Err(CommandError::rejected(format!(
            "branch {} has no commits that are not on {}; commit the work before opening a pull request",
            checkout.branch, checkout.base
        )));
    }
    let origin = git.origin_url(&checkout.location, &checkout.worktree)?;
    let repository = GitHubRepository::from_remote_url(&origin).ok_or_else(|| {
        CommandError::rejected(format!(
            "remote origin {origin:?} is not a GitHub repository; pull requests are opened on GitHub only"
        ))
    })?;
    git.push_branch(&checkout.location, &checkout.worktree, &checkout.branch)?;
    let pull_request_url = match github.find_open_pull_request(&repository, &checkout.branch)? {
        Some(url) => url,
        None => github.create_pull_request(&NewPullRequest {
            repository: &repository,
            head: &checkout.branch,
            base: &checkout.base,
            title: &checkout.task.title,
            body: &checkout.task.description,
        })?,
    };
    Ok(lock(storage).open_pull_request(id, &records, &pull_request_url)?)
}

fn complete(
    storage: &StorageState,
    github: &GitHub,
    id: model::TaskId,
) -> CommandResult<model::Task> {
    let task = lock(storage)
        .get_task(id)?
        .ok_or(StorageError::TaskNotFound(id))?;
    let pull_request_url = match (task.status, task.pull_request_url) {
        (TaskStatus::InReview, Some(url)) => url,
        (status, _) => {
            return Err(CommandError::rejected(format!(
                "task {id} is {status} without a pull request; only an in_review task can be completed"
            )));
        }
    };
    match github.pull_request_state(&pull_request_url)? {
        PullRequestState::Merged => {}
        PullRequestState::Open => {
            return Err(CommandError::rejected(format!(
                "pull request {pull_request_url} is not merged yet; merge it before completing the task"
            )));
        }
        PullRequestState::Closed => {
            return Err(CommandError::rejected(format!(
                "pull request {pull_request_url} was closed without merging; reopen it or cancel the task"
            )));
        }
    }
    let mut storage = lock(storage);
    storage.set_task_status(id, TaskStatus::Done)?;
    Ok(storage
        .get_task(id)?
        .ok_or(StorageError::TaskNotFound(id))?)
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
        assert_eq!(
            stored(&fixture, &fixture.dependent).status,
            TaskStatus::Planned
        );
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
        let git_runner = runner(vec![log(&[])]);
        let gh_runner = runner(Vec::new());
        let (git, github) = adapters(&git_runner, &gh_runner);

        let error = open(&fixture.storage, &git, &github, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error
                .message
                .contains("has no commits that are not on main")
        );
        assert_eq!(git_commands(&git_runner), ["log"]);
        assert!(invocations(&gh_runner).is_empty());
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
            Ok(format!(r#"[{{"url":"{PR}"}}]"#)),
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
            .open_pull_request(fixture.task.id, &[], PR)
            .unwrap();
        fixture
    }

    fn state(state: &str) -> Result<String, GitError> {
        Ok(format!(r#"{{"state":"{state}"}}"#))
    }

    #[test]
    fn completing_a_task_with_a_merged_pull_request_finishes_it_and_releases_dependents() {
        let fixture = in_review_fixture();
        let gh_runner = runner(vec![state("MERGED")]);
        let github = GitHub::new(Box::new(gh_runner.clone()));

        let completed = complete(&fixture.storage, &github, fixture.task.id).unwrap();

        assert_eq!(completed.status, TaskStatus::Done);
        assert_eq!(completed.pull_request_url.as_deref(), Some(PR));
        assert_eq!(stored(&fixture, &fixture.task), completed);
        assert_eq!(
            stored(&fixture, &fixture.dependent).status,
            TaskStatus::Ready
        );
        assert_eq!(
            invocations(&gh_runner)[0].args,
            ["pr", "view", PR, "--json", "state"].map(str::to_owned)
        );
    }

    #[test]
    fn a_pull_request_that_is_not_merged_keeps_the_task_in_review() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![state("OPEN"), state("CLOSED")])));

        for expected in ["is not merged yet", "was closed without merging"] {
            let error = complete(&fixture.storage, &github, fixture.task.id).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::Rejected);
            assert!(error.message.contains(expected), "{}", error.message);
        }
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert_eq!(
            stored(&fixture, &fixture.dependent).status,
            TaskStatus::Planned
        );
    }

    #[test]
    fn only_an_in_review_task_can_be_completed() {
        let fixture = local_fixture();
        let gh_runner = runner(Vec::new());
        let github = GitHub::new(Box::new(gh_runner.clone()));

        let error = complete(&fixture.storage, &github, fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("only an in_review task"));
        assert!(invocations(&gh_runner).is_empty());
        let error = complete(&fixture.storage, &github, model::TaskId(999)).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
    }
}
