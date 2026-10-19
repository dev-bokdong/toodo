//! Commands that finish a task (see `docs/workflow.md`): recording the commits of its
//! branch, opening its pull request, which pushes the branch, opens (or reuses) a pull
//! request with `gh`, and moves the task to `in_review`, and completing it once that
//! pull request is merged, which closes its herdr workspace (keeping the worktree), moves
//! it to `done`, and releases its dependents.
//!
//! They run on the async runtime because git and gh can take seconds. The database lock
//! is not held while they run.
//!
//! A background thread also moves tasks on its own every minute, telling the frontend
//! through [`TasksChanged`]: it looks up the pull request of the branch of each
//! `in_progress` task and moves the task to `in_review` when one was opened outside
//! Toodo (e.g. by an agent in herdr), then checks the pull requests of `in_review` tasks
//! and completes each task whose pull request was merged, as the `complete_task` command
//! would. For each pull request still open, it keeps the CI and review state and tells
//! the frontend through [`PullRequestStatusesChanged`] when one changes.
//!
//! [`TasksChanged`]: super::events::TasksChanged
//! [`PullRequestStatusesChanged`]: super::events::PullRequestStatusesChanged

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use super::{Runtime, State};

use super::events::Notifier;
use super::herdr::{WORKSPACE_NOT_FOUND, has_code};
use super::types::{self, CommitRecord, Task, TaskId};
use super::{CommandError, CommandResult, GitHubState, GitState, HerdrState, StorageState, lock};
use crate::adapters::git::Git;
use crate::adapters::github::{
    GitHub, NewPullRequest, PullRequest, PullRequestState, PullRequestStatus,
};
use crate::adapters::herdr::Herdr;
use crate::domain::issue::{closes_issue, with_closing_reference};
use crate::domain::model::{self, WorkspaceLocation};
use crate::domain::remote::GitHubRepository;
use crate::domain::status::TaskStatus;
use crate::storage::{Storage, StorageError};

/// Reads the commits of an `in_progress` or `in_review` task's branch that are not on
/// the base branch and stores them as its commit records.
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
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
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn open_pull_request(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    github: State<'_, GitHubState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&open(&storage, &git, &github, id.into())?).into())
}

/// Moves an `in_review` task to `done` once its pull request is merged, which may make
/// its dependents `ready`. The size, reviews, and commits of the pull request are
/// recorded for reviews. Its herdr workspace is closed; the worktree stays on disk.
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn complete_task(
    storage: State<'_, StorageState>,
    github: State<'_, GitHubState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
) -> CommandResult<Task> {
    Ok((&complete(&storage, &github, &herdr, id.into())?).into())
}

/// Concludes an `in_progress` or `in_review` task without merging, e.g. research whose
/// result is not adopted: the task becomes `done` as `not_adopted` with `note`, the
/// conclusion, as its completion note. The commits of its branch are recorded, an open
/// pull request of its branch, recorded or not, is closed with the conclusion as a
/// comment and recorded, and its herdr workspace is closed. The branch and worktree stay,
/// and its dependents keep waiting.
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn conclude_task(
    storage: State<'_, StorageState>,
    git: State<'_, GitState>,
    github: State<'_, GitHubState>,
    herdr: State<'_, HerdrState>,
    id: TaskId,
    note: String,
) -> CommandResult<Task> {
    Ok((&conclude(
        &storage,
        &git,
        &github,
        &herdr,
        id.into(),
        &note,
        OpenPullRequest::Close,
    )?)
        .into())
}

/// What concluding does with a pull request of the task that is still open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenPullRequest {
    /// Close it with the conclusion as a comment, as the app does.
    Close,
    /// Reject the conclusion and leave the pull request and the task as they are, as
    /// agents do through MCP: closing a pull request stays a decision for the user.
    Reject,
}

/// An `in_progress` or `in_review` task with the workspace and worktree its git commands
/// run in.
pub(super) struct Checkout {
    pub(super) task: model::Task,
    pub(super) location: WorkspaceLocation,
    pub(super) base: String,
    pub(super) branch: String,
    pub(super) worktree: String,
}

pub(super) fn checkout(storage: &StorageState, id: model::TaskId) -> CommandResult<Checkout> {
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
    let issue = checkout.task.issue.as_ref();
    let pull_request_url = match existing {
        Some(pull_request) => {
            if let Some(issue) = issue {
                ensure_closing_reference(github, &pull_request.url, issue)?;
            }
            pull_request.url
        }
        None => {
            let body = match issue {
                Some(issue) => with_closing_reference(&checkout.task.description, issue.number),
                None => checkout.task.description.clone(),
            };
            github.create_pull_request(&NewPullRequest {
                repository: &repository,
                head: &checkout.branch,
                base: &checkout.base,
                title: &checkout.task.title,
                body: &body,
            })?
        }
    };
    Ok(lock(storage).open_pull_request(id, &records, &pull_request_url, None)?)
}

/// Appends a closing reference to `issue` to the body of the open pull request at `url`
/// unless the body already has one, so merging the pull request closes the issue.
pub(super) fn ensure_closing_reference(
    github: &GitHub,
    url: &str,
    issue: &model::LinkedIssue,
) -> CommandResult<()> {
    let body = github.pull_request_body(url)?;
    if !closes_issue(&body, issue.number) {
        github.edit_pull_request_body(url, &with_closing_reference(&body, issue.number))?;
    }
    Ok(())
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

pub(crate) fn complete(
    storage: &StorageState,
    github: &GitHub,
    herdr: &Herdr,
    id: model::TaskId,
) -> CommandResult<model::Task> {
    match complete_if_merged(storage, github, herdr, id)? {
        Completion::Completed(task) => Ok(*task),
        Completion::Open(url, _) => Err(CommandError::rejected(format!(
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
    /// The pull request at this URL is still open, with this CI and review state; the
    /// task stays `in_review`.
    Open(String, PullRequestStatus),
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
        (TaskStatus::Done, _) => {
            let resolution = task
                .resolution
                .map_or("done", model::TaskResolution::as_str);
            return Err(CommandError::rejected(format!(
                "task {id} is already done as {resolution}; there is nothing left to complete"
            )));
        }
        (status, Some(_)) => {
            return Err(CommandError::rejected(format!(
                "task {id} is {status}; only an in_review task can be completed"
            )));
        }
        (status, None) => {
            return Err(CommandError::rejected(format!(
                "task {id} is {status} without a pull request; only an in_review task can be completed"
            )));
        }
    };
    let viewed = github.pull_request(&pull_request_url)?;
    let pull_request = viewed.pull_request;
    check_base(&pull_request, &base)?;
    match pull_request.state {
        PullRequestState::Merged => {}
        PullRequestState::Open => {
            // Pull requests opened outside Toodo, e.g. by an agent, get the reference here.
            if let Some(issue) = &task.issue {
                ensure_closing_reference(github, &pull_request_url, issue)?;
            }
            return Ok(Completion::Open(pull_request_url, viewed.status));
        }
        PullRequestState::Closed => return Ok(Completion::Closed(pull_request_url)),
    }
    // Read before anything changes: if `gh` fails, the task stays `in_review` and the
    // next check tries again, so a completed task never lacks its snapshot.
    let merged = github.merged_pull_request(&pull_request_url)?;
    let commits: Vec<_> = merged
        .commits
        .into_iter()
        .map(|commit| model::CommitRecord {
            task_id: id,
            sha: commit.sha,
            message: commit.message,
            author: commit.author,
            committed_at: commit.committed_at,
        })
        .collect();
    // A workspace herdr no longer has was already closed, e.g. by the user.
    if let Some(workspace_id) = &task.herdr_workspace_id {
        match herdr.close_workspace(&location.machine(), workspace_id) {
            Ok(()) => {}
            Err(error) if has_code(&error, WORKSPACE_NOT_FOUND) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Completion::Completed(Box::new(
        lock(storage).complete_task(
            id,
            pull_request.merged_at,
            Some(&merged.snapshot),
            &commits,
        )?,
    )))
}

pub(crate) fn conclude(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    herdr: &Herdr,
    id: model::TaskId,
    note: &str,
    open_pull_request: OpenPullRequest,
) -> CommandResult<model::Task> {
    let note = note.trim();
    if note.is_empty() {
        return Err(CommandError::invalid_input(
            "a conclusion must say why the work is not adopted",
        ));
    }
    let (task, workspace) = {
        let storage = lock(storage);
        let task = storage
            .get_task(id)?
            .ok_or(StorageError::TaskNotFound(id))?;
        let workspace = storage
            .get_workspace(task.workspace_id)?
            .ok_or(StorageError::WorkspaceNotFound(task.workspace_id))?;
        (task, workspace)
    };
    if !matches!(task.status, TaskStatus::InProgress | TaskStatus::InReview) {
        return Err(CommandError::rejected(format!(
            "task {id} is {}; only an in_progress or in_review task can be concluded without merging",
            task.status
        )));
    }
    // A branch whose log is empty, or a task without a worktree, keeps the commits
    // recorded before.
    let records = match (&task.branch, &task.worktree_path) {
        (Some(branch), Some(worktree)) => commit_records(
            git,
            &Checkout {
                task: task.clone(),
                location: workspace.location.clone(),
                base: workspace.default_base_branch.clone(),
                branch: branch.clone(),
                worktree: worktree.to_string(),
            },
        )?,
        _ => Vec::new(),
    };
    // The pull requests of the branch on GitHub decide, not only the recorded one: an
    // agent may have opened one that the background check has not adopted yet.
    let pull_requests = match &task.branch {
        Some(branch) => branch_pull_requests(git, github, &workspace, branch)?,
        None => Vec::new(),
    };
    if let Some(merged) = pull_requests
        .iter()
        .find(|pull_request| pull_request.state == PullRequestState::Merged)
    {
        return Err(CommandError::rejected(format!(
            "pull request {} was merged; the task completes with the merge instead",
            merged.url
        )));
    }
    let closed = match pull_requests
        .iter()
        .find(|pull_request| pull_request.state == PullRequestState::Open)
    {
        None => None,
        Some(open) => match open_pull_request {
            OpenPullRequest::Close => {
                github.close_pull_request(&open.url, note)?;
                Some(open.url.as_str())
            }
            OpenPullRequest::Reject => {
                return Err(CommandError::rejected(format!(
                    "pull request {} is still open; concluding through MCP never closes a pull request, so the user concludes this task in the Toodo app, which closes it with the conclusion as a comment",
                    open.url
                )));
            }
        },
    };
    // A workspace herdr no longer has was already closed, e.g. by the user.
    if let Some(workspace_id) = &task.herdr_workspace_id {
        match herdr.close_workspace(&workspace.location.machine(), workspace_id) {
            Ok(()) => {}
            Err(error) if has_code(&error, WORKSPACE_NOT_FOUND) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(lock(storage).conclude_task(id, note, &records, closed)?)
}

/// The pull requests from `branch` into the base branch of `workspace` on GitHub, newest
/// first. A repository without an `origin` remote, or whose `origin` is not on GitHub,
/// has none.
fn branch_pull_requests(
    git: &Git,
    github: &GitHub,
    workspace: &model::Workspace,
    branch: &str,
) -> CommandResult<Vec<PullRequest>> {
    let location = &workspace.location;
    if !git.has_origin(location)? {
        return Ok(Vec::new());
    }
    let path = match location {
        WorkspaceLocation::Local { path } => path.as_str(),
        WorkspaceLocation::Ssh { path, .. } => path.as_str(),
    };
    let origin = git.origin_url(location, path)?;
    let Some(repository) = GitHubRepository::from_remote_url(&origin) else {
        return Ok(Vec::new());
    };
    Ok(github.pull_requests_into(&repository, branch, &workspace.default_base_branch)?)
}

/// Moves an `in_progress` task to `in_review` when its branch has an open or merged
/// pull request into the workspace base branch, which was opened outside Toodo (e.g. by
/// an agent in herdr). Unlike opening a pull request, nothing is pushed. Returns the
/// task if it moved; a branch without a pull request, or with a closed one only, leaves
/// it `in_progress`.
fn adopt_pull_request(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    id: model::TaskId,
) -> CommandResult<Option<model::Task>> {
    let checkout = checkout(storage, id)?;
    if checkout.task.status != TaskStatus::InProgress {
        return Ok(None);
    }
    let origin = git.origin_url(&checkout.location, &checkout.worktree)?;
    let repository = GitHubRepository::from_remote_url(&origin).ok_or_else(|| {
        CommandError::rejected(format!(
            "remote origin {origin:?} is not a GitHub repository; pull requests are looked up on GitHub only"
        ))
    })?;
    let Some(pull_request) = github
        .find_pull_request(&repository, &checkout.branch)?
        .filter(|pull_request| pull_request.state != PullRequestState::Closed)
    else {
        return Ok(None);
    };
    check_base(&pull_request, &checkout.base)?;
    let records = commit_records(git, &checkout)?;
    let mut storage = lock(storage);
    // The task may have been paused while git and gh ran.
    let task = storage
        .get_task(id)?
        .ok_or(StorageError::TaskNotFound(id))?;
    if task.status != TaskStatus::InProgress {
        return Ok(None);
    }
    // After a merge the base may already contain the branch, so its log can be empty;
    // keep the commits recorded before in that case.
    let records = if records.is_empty() && pull_request.state == PullRequestState::Merged {
        storage.list_commit_records(id)?
    } else {
        records
    };
    Ok(Some(storage.open_pull_request(
        id,
        &records,
        &pull_request.url,
        pull_request.merged_at,
    )?))
}

/// CI and review state of the open pull request of each `in_review` task, as of the last
/// check.
pub type PullRequestStatusState = Mutex<HashMap<model::TaskId, PullRequestStatus>>;

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_pull_request_statuses(
    statuses: State<'_, PullRequestStatusState>,
) -> Vec<types::TaskPullRequestStatus> {
    all_statuses(&lock_statuses(&statuses))
}

fn lock_statuses(
    statuses: &PullRequestStatusState,
) -> MutexGuard<'_, HashMap<model::TaskId, PullRequestStatus>> {
    statuses.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The pull request statuses, ordered by task ID.
fn all_statuses(
    statuses: &HashMap<model::TaskId, PullRequestStatus>,
) -> Vec<types::TaskPullRequestStatus> {
    let mut all: Vec<_> = statuses
        .iter()
        .map(|(&id, &status)| types::TaskPullRequestStatus::new(id, status))
        .collect();
    all.sort_by_key(|status| status.task_id.0);
    all
}

/// How often the pull requests of `in_progress` and `in_review` tasks are checked.
const PULL_REQUEST_CHECK_PERIOD: Duration = Duration::from_secs(60);

/// Starts the background thread that moves `in_progress` tasks whose branch has a pull
/// request to `in_review` and completes `in_review` tasks whose pull request was merged.
pub fn spawn(app: Arc<dyn Runtime>) -> io::Result<()> {
    thread::Builder::new()
        .name("pull-request-checks".to_owned())
        .spawn(move || {
            let mut failures = HashMap::new();
            loop {
                check_pull_requests(
                    app.storage(),
                    app.git(),
                    app.github(),
                    app.herdr(),
                    app.as_ref(),
                    &mut failures,
                    app.pull_request_statuses(),
                );
                thread::sleep(PULL_REQUEST_CHECK_PERIOD);
            }
        })?;
    Ok(())
}

/// Moves every `in_progress` task whose branch has a pull request to `in_review`, then
/// completes every `in_review` task whose pull request was merged (including the ones
/// just moved), and announces each change. `failures` holds the last failure of each
/// task, so a failure that repeats on every check (e.g. `gh` is not signed in) is logged
/// once. `statuses` gets the CI and review state of each pull request still open; a
/// failed check keeps the last one known.
fn check_pull_requests(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    herdr: &Herdr,
    notifier: &dyn Notifier,
    failures: &mut HashMap<model::TaskId, String>,
    statuses: &PullRequestStatusState,
) {
    let in_progress = match in_progress_tasks(&lock(storage)) {
        Ok(tasks) => tasks,
        Err(error) => {
            eprintln!("toodo: failed to list tasks in progress: {error}");
            return;
        }
    };
    for &id in &in_progress {
        match adopt_pull_request(storage, git, github, id) {
            Ok(adopted) => {
                failures.remove(&id);
                if let Some(task) = adopted {
                    notifier.tasks_changed(task.workspace_id);
                }
            }
            Err(error) => {
                remember_failure(failures, id, "look up its pull request", error);
            }
        }
    }
    let in_review = match in_review_tasks(&lock(storage)) {
        Ok(tasks) => tasks,
        Err(error) => {
            eprintln!("toodo: failed to list tasks in review: {error}");
            return;
        }
    };
    failures.retain(|id, _| in_progress.contains(id) || in_review.contains(id));
    let before = lock_statuses(statuses).clone();
    let mut after = before.clone();
    after.retain(|id, _| in_review.contains(id));
    for id in in_review {
        match complete_if_merged(storage, github, herdr, id) {
            Ok(completion) => {
                failures.remove(&id);
                match completion {
                    Completion::Completed(task) => {
                        after.remove(&id);
                        notifier.tasks_changed(task.workspace_id);
                    }
                    Completion::Open(_, status) => {
                        after.insert(id, status);
                    }
                    Completion::Closed(_) => {
                        after.remove(&id);
                    }
                }
            }
            Err(error) => {
                remember_failure(
                    failures,
                    id,
                    "complete it with a merged pull request",
                    error,
                );
            }
        }
    }
    if after != before {
        notifier.pull_request_statuses_changed(&all_statuses(&after));
        *lock_statuses(statuses) = after;
    }
}

/// Logs a failed check of a task unless it is the same failure as the last one.
fn remember_failure(
    failures: &mut HashMap<model::TaskId, String>,
    id: model::TaskId,
    action: &str,
    error: CommandError,
) {
    if failures.get(&id) != Some(&error.message) {
        eprintln!("toodo: task {id}: failed to {action}: {}", error.message);
        failures.insert(id, error.message);
    }
}

/// IDs of the `in_progress` tasks of all workspaces that have a branch and a worktree.
fn in_progress_tasks(storage: &Storage) -> Result<Vec<model::TaskId>, StorageError> {
    Ok(storage
        .list_all_tasks()?
        .into_iter()
        .filter(|task| {
            task.status == TaskStatus::InProgress
                && task.branch.is_some()
                && task.worktree_path.is_some()
        })
        .map(|task| task.id)
        .collect())
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
            estimate: None,
            due_date: None,
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
                path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
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

    /// A git that must not be called.
    fn no_git() -> Git {
        Git::new(Box::new(runner(Vec::new())))
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
        viewed_with(state, base, "[]", "\"\"")
    }

    /// `gh pr view` output with these `statusCheckRollup` and `reviewDecision` values.
    fn viewed_with(
        state: &str,
        base: &str,
        checks: &str,
        decision: &str,
    ) -> Result<String, GitError> {
        let mut view: serde_json::Value =
            serde_json::from_str(&pull_request_json(state, base)).unwrap();
        view["statusCheckRollup"] = serde_json::from_str(checks).unwrap();
        view["reviewDecision"] = serde_json::from_str(decision).unwrap();
        view["latestReviews"] = serde_json::json!([]);
        Ok(view.to_string())
    }

    /// `gh pr view` output with the size, reviews, and commits of the merged pull
    /// request, which has the commits `shas`.
    fn merged_details(shas: &[&str]) -> Result<String, GitError> {
        let commits = shas
            .iter()
            .map(|sha| {
                format!(
                    r#"{{"authors":[{{"email":"ada@example.com","name":"Ada"}}],"committedDate":"{}","messageBody":"","messageHeadline":"Commit {sha}","oid":"{sha}"}}"#,
                    MERGED_AT.0
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        Ok(format!(
            r#"{{"additions":10,"author":{{"login":"ada"}},"changedFiles":2,"commits":[{commits}],"deletions":3,"mergeCommit":{{"oid":"{SHA_B}"}},"number":12,"reviews":[{{"author":{{"login":"bob"}},"state":"APPROVED","submittedAt":"{}"}}],"title":"Add login"}}"#,
            MERGED_AT.0
        ))
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
        let gh_runner = runner(vec![
            listed("MERGED", "main"),
            viewed("MERGED", "main"),
            merged_details(&[]),
        ]);
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
        // A pull request without commits keeps the commits recorded before.
        assert_eq!(recorded_shas(&fixture), [SHA_A]);
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
        let gh_runner = runner(vec![
            viewed("MERGED", "main"),
            merged_details(&[SHA_A, SHA_B]),
        ]);
        let github = GitHub::new(Box::new(gh_runner.clone()));
        let (herdr, herdr_runner) = closing_herdr();

        let completed = complete(&fixture.storage, &github, &herdr, fixture.task.id).unwrap();

        assert_eq!(
            completed.pull_request,
            Some(model::PullRequestSnapshot {
                number: 12,
                title: "Add login".to_owned(),
                additions: 10,
                deletions: 3,
                changed_files: 2,
                merge_commit_sha: Some(SHA_B.to_owned()),
                review_count: 1,
                reviewer_count: 1,
                first_reviewed_at: Some(MERGED_AT.1),
            })
        );
        // The commits pushed after the pull request was opened are recorded too.
        assert_eq!(recorded_shas(&fixture), [SHA_A, SHA_B]);
        assert_eq!(completed.completion_note, None);

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
            [
                "pr",
                "view",
                PR,
                "--json",
                "url,state,baseRefName,mergedAt,statusCheckRollup,reviewDecision,latestReviews"
            ]
            .map(str::to_owned)
        );
        assert_eq!(invocations(&gh_runner)[1].args[..3], ["pr", "view", PR]);
    }

    #[test]
    fn a_failure_to_read_the_merged_pull_request_keeps_the_task_in_review() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![
            viewed("MERGED", "main"),
            Err(failed()),
        ])));
        let before = stored(&fixture, &fixture.task);

        complete(&fixture.storage, &github, &no_herdr(), fixture.task.id).unwrap_err();

        assert_eq!(stored(&fixture, &fixture.task), before);
        assert!(!dependent_ready(&fixture));
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
        let github = GitHub::new(Box::new(runner(vec![
            viewed("MERGED", "main"),
            merged_details(&[]),
        ])));
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

    #[test]
    fn concluding_closes_the_open_pull_request_and_keeps_dependents_waiting() {
        let fixture = in_review_fixture();
        let git_runner = runner(vec![
            log(&[SHA_A, SHA_B]),
            Ok("origin\n".to_owned()),
            origin("git@github.com:acme/toodo.git"),
        ]);
        let gh_runner = runner(vec![listed("OPEN", "main"), Ok(String::new())]);
        let (git, github) = adapters(&git_runner, &gh_runner);
        let (herdr, herdr_runner) = closing_herdr();

        let concluded = conclude(
            &fixture.storage,
            &git,
            &github,
            &herdr,
            fixture.task.id,
            "  Twice as slow as the current cache\n",
            OpenPullRequest::Close,
        )
        .unwrap();

        assert_eq!(concluded.status, TaskStatus::Done);
        assert_eq!(
            concluded.resolution,
            Some(model::TaskResolution::NotAdopted)
        );
        assert_eq!(
            concluded.completion_note.as_deref(),
            Some("Twice as slow as the current cache")
        );
        assert_eq!(
            concluded.timeline.last().map(|event| event.status),
            Some(TaskStatus::Done)
        );
        assert_eq!(concluded.branch.as_deref(), Some("feat/add-login-1"));
        assert_eq!(concluded.worktree_path, fixture.task.worktree_path);
        assert_eq!(concluded.pull_request_url.as_deref(), Some(PR));
        assert_eq!(concluded.herdr_workspace_id, None);
        assert_eq!(stored(&fixture, &fixture.task), concluded);
        assert_eq!(recorded_shas(&fixture), [SHA_A, SHA_B]);
        assert_eq!(git_commands(&git_runner), ["log", "remote", "remote"]);
        assert_eq!(
            invocations(&gh_runner)[0].args[..8],
            [
                "pr",
                "list",
                "--repo",
                "github.com/acme/toodo",
                "--head",
                "feat/add-login-1",
                "--base",
                "main"
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            invocations(&gh_runner)[1].args,
            [
                "pr",
                "close",
                PR,
                "--comment",
                "Twice as slow as the current cache"
            ]
            .map(str::to_owned)
        );
        assert_eq!(
            invocations(&herdr_runner)[0].args,
            ["workspace", "close", "w27"].map(str::to_owned)
        );
        assert!(!dependent_ready(&fixture));
    }

    #[test]
    fn concluding_an_in_progress_task_whose_branch_has_no_pull_request() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            log(&[]),
            Ok("origin\n".to_owned()),
            origin("git@github.com:acme/toodo.git"),
        ]);
        let gh_runner = runner(vec![Ok("[]\n".to_owned())]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        let concluded = conclude(
            &fixture.storage,
            &git,
            &github,
            &closing_herdr().0,
            fixture.task.id,
            "Not worth it",
            OpenPullRequest::Close,
        )
        .unwrap();

        assert_eq!(
            concluded.resolution,
            Some(model::TaskResolution::NotAdopted)
        );
        assert_eq!(concluded.pull_request_url, None);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert!(recorded_shas(&fixture).is_empty());
        assert!(!dependent_ready(&fixture));

        // A done task cannot be concluded again.
        let error = conclude(
            &fixture.storage,
            &no_git(),
            &github,
            &no_herdr(),
            fixture.task.id,
            "Again",
            OpenPullRequest::Close,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
    }

    #[test]
    fn concluding_a_task_whose_pull_request_was_closed_does_not_close_it_again() {
        let fixture = in_review_fixture();
        let gh_runner = runner(vec![listed("CLOSED", "main")]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);

        let concluded = conclude(
            &fixture.storage,
            &git,
            &github,
            &closing_herdr().0,
            fixture.task.id,
            "Superseded",
            OpenPullRequest::Close,
        )
        .unwrap();

        assert_eq!(concluded.status, TaskStatus::Done);
        assert_eq!(invocations(&gh_runner).len(), 1);
    }

    #[test]
    fn concluding_without_closing_rejects_an_open_pull_request_and_changes_nothing() {
        let fixture = in_review_fixture();
        let before = stored(&fixture, &fixture.task);
        let gh_runner = runner(vec![listed("OPEN", "main")]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);

        let error = conclude(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            fixture.task.id,
            "Not adopted",
            OpenPullRequest::Reject,
        )
        .unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("is still open"), "{}", error.message);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(stored(&fixture, &fixture.task), before);
        assert!(recorded_shas(&fixture).is_empty());
    }

    #[test]
    fn concluding_is_rejected_without_a_note_for_planned_tasks_and_for_merged_pull_requests() {
        let fixture = in_review_fixture();
        let before = stored(&fixture, &fixture.task);

        let error = conclude(
            &fixture.storage,
            &no_git(),
            &GitHub::new(Box::new(runner(Vec::new()))),
            &no_herdr(),
            fixture.task.id,
            " \n",
            OpenPullRequest::Close,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);

        let error = conclude(
            &fixture.storage,
            &no_git(),
            &GitHub::new(Box::new(runner(Vec::new()))),
            &no_herdr(),
            fixture.dependent.id,
            "Not needed",
            OpenPullRequest::Close,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error
                .message
                .contains("only an in_progress or in_review task")
        );

        let gh_runner = runner(vec![listed("MERGED", "main")]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);
        let error = conclude(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            fixture.task.id,
            "Not adopted",
            OpenPullRequest::Close,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("was merged"), "{}", error.message);
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(stored(&fixture, &fixture.task), before);
    }

    /// A git whose branch log has `shas` and whose `origin` is on GitHub.
    fn github_checkout(shas: &[&str]) -> Arc<FakeRunner> {
        runner(vec![
            log(shas),
            Ok("origin\n".to_owned()),
            origin("git@github.com:acme/toodo.git"),
        ])
    }

    #[test]
    fn concluding_finds_an_open_pull_request_that_was_not_recorded_yet() {
        let fixture = local_fixture();
        let before = stored(&fixture, &fixture.task);
        assert_eq!(before.pull_request_url, None);

        // Through MCP the conclusion is rejected and nothing changes.
        let gh_runner = runner(vec![listed("OPEN", "main")]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);
        let error = conclude(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            fixture.task.id,
            "Not adopted",
            OpenPullRequest::Reject,
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains(&format!("{PR} is still open")),
            "{}",
            error.message
        );
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(stored(&fixture, &fixture.task), before);
        assert!(recorded_shas(&fixture).is_empty());

        // The app closes it with the conclusion, records it, and concludes.
        let gh_runner = runner(vec![listed("OPEN", "main"), Ok(String::new())]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);
        let concluded = conclude(
            &fixture.storage,
            &git,
            &github,
            &closing_herdr().0,
            fixture.task.id,
            "Not adopted",
            OpenPullRequest::Close,
        )
        .unwrap();
        assert_eq!(concluded.status, TaskStatus::Done);
        assert_eq!(
            concluded.resolution,
            Some(model::TaskResolution::NotAdopted)
        );
        assert_eq!(concluded.pull_request_url.as_deref(), Some(PR));
        assert_eq!(stored(&fixture, &fixture.task), concluded);
        assert_eq!(
            invocations(&gh_runner)[1].args,
            ["pr", "close", PR, "--comment", "Not adopted"].map(str::to_owned)
        );
    }

    #[test]
    fn concluding_is_rejected_for_a_merged_pull_request_that_was_not_recorded_yet() {
        let fixture = local_fixture();
        let before = stored(&fixture, &fixture.task);
        for open_pull_request in [OpenPullRequest::Reject, OpenPullRequest::Close] {
            let gh_runner = runner(vec![listed("MERGED", "main")]);
            let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);
            let error = conclude(
                &fixture.storage,
                &git,
                &github,
                &no_herdr(),
                fixture.task.id,
                "Not adopted",
                open_pull_request,
            )
            .unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::Rejected);
            assert!(
                error.message.contains("completes with the merge"),
                "{}",
                error.message
            );
            assert_eq!(invocations(&gh_runner).len(), 1);
            assert_eq!(stored(&fixture, &fixture.task), before);
        }
    }

    #[test]
    fn concluding_skips_gh_for_a_repository_without_origin() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[]), Ok(String::new())]);
        let gh_runner = runner(Vec::new());
        let (git, github) = adapters(&git_runner, &gh_runner);

        let concluded = conclude(
            &fixture.storage,
            &git,
            &github,
            &closing_herdr().0,
            fixture.task.id,
            "Not worth it",
            OpenPullRequest::Reject,
        )
        .unwrap();

        assert_eq!(concluded.status, TaskStatus::Done);
        assert_eq!(git_commands(&git_runner), ["log", "remote"]);
        assert!(invocations(&gh_runner).is_empty());
    }

    #[test]
    fn concluding_fails_and_changes_nothing_when_the_lookup_fails() {
        let fixture = in_review_fixture();
        let before = stored(&fixture, &fixture.task);
        let gh_runner = runner(vec![Err(GitError::Failed {
            command: "gh pr list".to_owned(),
            stderr: "error connecting to api.github.com".to_owned(),
        })]);
        let (git, github) = adapters(&github_checkout(&[SHA_A]), &gh_runner);

        let error = conclude(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            fixture.task.id,
            "Not adopted",
            OpenPullRequest::Close,
        )
        .unwrap_err();

        assert!(
            error.message.contains("error connecting to api.github.com"),
            "{}",
            error.message
        );
        assert_eq!(invocations(&gh_runner).len(), 1);
        assert_eq!(stored(&fixture, &fixture.task), before);
        assert!(recorded_shas(&fixture).is_empty());
    }

    #[test]
    fn completing_a_done_task_says_how_it_was_done() {
        let fixture = local_fixture();
        let git_runner = runner(vec![log(&[]), Ok(String::new())]);
        let (git, github) = adapters(&git_runner, &runner(Vec::new()));
        conclude(
            &fixture.storage,
            &git,
            &github,
            &closing_herdr().0,
            fixture.task.id,
            "Not worth it",
            OpenPullRequest::Close,
        )
        .unwrap();

        let error = complete(&fixture.storage, &github, &no_herdr(), fixture.task.id).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains("is already done as not_adopted"),
            "{}",
            error.message
        );
    }

    #[derive(Default)]
    struct Recorder(
        Mutex<Vec<model::WorkspaceId>>,
        Mutex<Vec<Vec<types::TaskPullRequestStatus>>>,
    );

    impl Notifier for Recorder {
        fn tasks_changed(&self, workspace: model::WorkspaceId) {
            self.0.lock().unwrap().push(workspace);
        }

        fn workspaces_changed(&self) {}

        fn agent_statuses_changed(&self, _: &[types::TaskAgentStatus]) {}

        fn pull_request_statuses_changed(&self, statuses: &[types::TaskPullRequestStatus]) {
            self.1.lock().unwrap().push(statuses.to_vec());
        }
    }

    #[test]
    fn a_merged_pull_request_completes_its_task_automatically() {
        let fixture = in_review_fixture();
        let gh_runner = runner(vec![
            viewed("OPEN", "main"),
            viewed("MERGED", "main"),
            merged_details(&[]),
        ]);
        let github = GitHub::new(Box::new(gh_runner.clone()));
        let (herdr, herdr_runner) = closing_herdr();
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();

        check_pull_requests(
            &fixture.storage,
            &no_git(),
            &github,
            &herdr,
            &recorder,
            &mut failures,
            &statuses,
        );
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert!(recorder.0.lock().unwrap().is_empty());
        assert!(invocations(&herdr_runner).is_empty());

        check_pull_requests(
            &fixture.storage,
            &no_git(),
            &github,
            &herdr,
            &recorder,
            &mut failures,
            &statuses,
        );
        let done = stored(&fixture, &fixture.task);
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(done.pull_request_merged_at, Some(MERGED_AT.1));
        assert!(dependent_ready(&fixture));
        assert_eq!(*recorder.0.lock().unwrap(), [done.workspace_id]);
        assert_eq!(invocations(&herdr_runner).len(), 1);
        assert!(failures.is_empty());

        // A done task is no longer checked.
        check_pull_requests(
            &fixture.storage,
            &no_git(),
            &github,
            &herdr,
            &recorder,
            &mut failures,
            &statuses,
        );
        assert_eq!(invocations(&gh_runner).len(), 3);
    }

    #[test]
    fn checks_keep_the_ci_and_review_state_of_open_pull_requests_and_announce_changes() {
        let fixture = in_review_fixture();
        let running = r#"[{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}]"#;
        let failing = r#"[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}]"#;
        let github = GitHub::new(Box::new(runner(vec![
            viewed_with("OPEN", "main", running, "\"\""),
            viewed_with("OPEN", "main", running, "\"\""),
            viewed_with("OPEN", "main", failing, "\"CHANGES_REQUESTED\""),
            Err(failed()),
            viewed("MERGED", "main"),
            merged_details(&[]),
        ])));
        let (herdr, _) = closing_herdr();
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();
        let check = |failures: &mut HashMap<_, _>| {
            check_pull_requests(
                &fixture.storage,
                &no_git(),
                &github,
                &herdr,
                &recorder,
                failures,
                &statuses,
            );
        };
        let status = |checks, review| types::TaskPullRequestStatus {
            task_id: fixture.task.id.into(),
            checks: Some(checks),
            review,
        };
        let pending = status(types::ChecksStatus::Pending, types::ReviewStatus::Pending);
        let failed = status(
            types::ChecksStatus::Failing,
            types::ReviewStatus::ChangesRequested,
        );

        // An unchanged state is announced once; a failed check keeps the last one.
        for _ in 0..4 {
            check(&mut failures);
        }
        assert_eq!(
            *recorder.1.lock().unwrap(),
            [vec![pending], vec![failed.clone()]]
        );
        assert_eq!(list(&statuses), [failed]);

        // A merged pull request leaves no state behind.
        check(&mut failures);
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::Done);
        assert!(recorder.1.lock().unwrap().last().unwrap().is_empty());
        assert!(list(&statuses).is_empty());
    }

    fn list(statuses: &PullRequestStatusState) -> Vec<types::TaskPullRequestStatus> {
        all_statuses(&lock_statuses(statuses))
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
        let statuses = PullRequestStatusState::default();

        check_pull_requests(
            &fixture.storage,
            &no_git(),
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert_eq!(failures.keys().collect::<Vec<_>>(), [&fixture.task.id]);

        check_pull_requests(
            &fixture.storage,
            &no_git(),
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        assert!(failures.is_empty());
        assert!(recorder.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_pull_request_opened_outside_toodo_moves_an_in_progress_task_to_review() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            origin("git@github.com:acme/toodo.git"),
            origin("git@github.com:acme/toodo.git"),
            log(&[SHA_A]),
        ]);
        let gh_runner = runner(vec![
            Ok("[]".to_owned()),
            listed("OPEN", "main"),
            viewed("OPEN", "main"),
        ]);
        let (git, github) = adapters(&git_runner, &gh_runner);
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();

        check_pull_requests(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        assert_eq!(stored(&fixture, &fixture.task), fixture.task);
        assert!(recorder.0.lock().unwrap().is_empty());

        check_pull_requests(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        let in_review = stored(&fixture, &fixture.task);
        assert_eq!(in_review.status, TaskStatus::InReview);
        assert_eq!(in_review.pull_request_url.as_deref(), Some(PR));
        assert_eq!(in_review.pull_request_merged_at, None);
        assert_eq!(recorded_shas(&fixture), [SHA_A]);
        assert_eq!(*recorder.0.lock().unwrap(), [in_review.workspace_id]);
        assert!(!dependent_ready(&fixture));
        assert!(failures.is_empty());
        // Nothing is pushed: the pull request already has the branch.
        assert_eq!(git_commands(&git_runner), ["remote", "remote", "log"]);
        assert_eq!(
            invocations(&gh_runner)[1].args[..6],
            [
                "pr",
                "list",
                "--repo",
                "github.com/acme/toodo",
                "--head",
                "feat/add-login-1"
            ]
            .map(str::to_owned)
        );
        // The task is now checked as an in_review task.
        assert_eq!(
            invocations(&gh_runner)[2].args[..2],
            ["pr", "view"].map(str::to_owned)
        );
    }

    const ISSUE: &str = "https://github.com/acme/toodo/issues/12";

    fn link_issue(fixture: &Fixture) {
        lock(&fixture.storage)
            .set_task_issue(
                fixture.task.id,
                Some(&model::LinkedIssue {
                    number: 12,
                    url: ISSUE.to_owned(),
                }),
            )
            .unwrap();
    }

    fn body(text: &str) -> Result<String, GitError> {
        Ok(serde_json::json!({ "body": text }).to_string())
    }

    #[test]
    fn a_new_pull_request_of_a_task_with_an_issue_closes_it() {
        let fixture = local_fixture();
        link_issue(&fixture);
        let git_runner = runner(vec![
            log(&[SHA_A]),
            origin("git@github.com:acme/toodo.git"),
            Ok(String::new()),
        ]);
        let gh_runner = runner(vec![Ok("[]".to_owned()), Ok(format!("{PR}\n"))]);
        let (git, github) = adapters(&git_runner, &gh_runner);

        open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

        let create = &invocations(&gh_runner)[1].args;
        assert_eq!(create[1], "create");
        assert_eq!(create.last().unwrap(), "Implement Add login\n\nCloses #12");
        assert_eq!(invocations(&gh_runner).len(), 2);
    }

    #[test]
    fn an_adopted_open_pull_request_gets_a_closing_reference_unless_it_has_one() {
        for (existing, edited) in [
            ("Work by an agent", Some("Work by an agent\n\nCloses #12")),
            ("Work by an agent\n\nfixes #12", None),
        ] {
            let fixture = local_fixture();
            link_issue(&fixture);
            let git_runner = runner(vec![
                log(&[SHA_A]),
                origin("git@github.com:acme/toodo.git"),
                Ok(String::new()),
            ]);
            let gh_runner = runner(vec![
                listed("OPEN", "main"),
                body(existing),
                Ok(String::new()),
            ]);
            let (git, github) = adapters(&git_runner, &gh_runner);

            let in_review = open(&fixture.storage, &git, &github, fixture.task.id).unwrap();

            assert_eq!(in_review.status, TaskStatus::InReview);
            let calls = invocations(&gh_runner);
            assert_eq!(
                calls[1].args,
                ["pr", "view", PR, "--json", "body"].map(str::to_owned)
            );
            match edited {
                Some(edited) => assert_eq!(
                    calls[2].args,
                    ["pr", "edit", PR, "--body", edited].map(str::to_owned)
                ),
                None => assert_eq!(calls.len(), 2),
            }
        }
    }

    #[test]
    fn a_pull_request_opened_by_an_agent_gets_a_closing_reference_in_the_same_check() {
        let fixture = local_fixture();
        link_issue(&fixture);
        let git_runner = runner(vec![origin("git@github.com:acme/toodo.git"), log(&[SHA_A])]);
        let gh_runner = runner(vec![
            listed("OPEN", "main"),
            viewed("OPEN", "main"),
            body("Work by an agent"),
            Ok(String::new()),
            viewed("OPEN", "main"),
            body("Work by an agent\n\nCloses #12"),
        ]);
        let (git, github) = adapters(&git_runner, &gh_runner);
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();

        for _ in 0..2 {
            check_pull_requests(
                &fixture.storage,
                &git,
                &github,
                &no_herdr(),
                &recorder,
                &mut failures,
                &statuses,
            );
        }

        assert_eq!(stored(&fixture, &fixture.task).status, TaskStatus::InReview);
        assert!(failures.is_empty());
        let calls = invocations(&gh_runner);
        assert_eq!(
            calls[3].args,
            ["pr", "edit", PR, "--body", "Work by an agent\n\nCloses #12"].map(str::to_owned)
        );
        // The second check finds the reference and edits nothing.
        assert_eq!(calls.len(), 6);
    }

    #[test]
    fn an_in_progress_task_whose_pull_request_was_merged_is_completed_in_one_check() {
        let fixture = local_fixture();
        let git_runner = runner(vec![origin("git@github.com:acme/toodo.git"), log(&[])]);
        let gh_runner = runner(vec![
            listed("MERGED", "main"),
            viewed("MERGED", "main"),
            merged_details(&[]),
        ]);
        let (git, github) = adapters(&git_runner, &gh_runner);
        let (herdr, herdr_runner) = closing_herdr();
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();

        check_pull_requests(
            &fixture.storage,
            &git,
            &github,
            &herdr,
            &recorder,
            &mut failures,
            &statuses,
        );

        let done = stored(&fixture, &fixture.task);
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(done.pull_request_url.as_deref(), Some(PR));
        assert_eq!(done.pull_request_merged_at, Some(MERGED_AT.1));
        assert!(dependent_ready(&fixture));
        assert_eq!(invocations(&herdr_runner).len(), 1);
        assert_eq!(recorder.0.lock().unwrap().len(), 2);
        assert!(failures.is_empty());
    }

    #[test]
    fn a_closed_pull_request_or_one_into_another_base_keeps_the_task_in_progress() {
        let fixture = local_fixture();
        let git_runner = runner(vec![
            origin("git@github.com:acme/toodo.git"),
            origin("git@github.com:acme/toodo.git"),
        ]);
        let gh_runner = runner(vec![listed("CLOSED", "main"), listed("OPEN", "develop")]);
        let (git, github) = adapters(&git_runner, &gh_runner);
        let recorder = Recorder::default();
        let mut failures = HashMap::new();
        let statuses = PullRequestStatusState::default();

        check_pull_requests(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        assert!(failures.is_empty());
        check_pull_requests(
            &fixture.storage,
            &git,
            &github,
            &no_herdr(),
            &recorder,
            &mut failures,
            &statuses,
        );
        assert!(
            failures[&fixture.task.id].contains("merges into develop"),
            "{failures:?}"
        );

        assert_eq!(stored(&fixture, &fixture.task), fixture.task);
        assert!(recorder.0.lock().unwrap().is_empty());
        assert_eq!(git_commands(&git_runner), ["remote", "remote"]);
    }

    #[test]
    fn a_herdr_failure_keeps_the_task_in_review() {
        let fixture = in_review_fixture();
        let github = GitHub::new(Box::new(runner(vec![
            viewed("MERGED", "main"),
            merged_details(&[SHA_A]),
        ])));
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
