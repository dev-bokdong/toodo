//! What the MCP tools do, over the same storage as the Tauri commands, so agents follow
//! the rules the UI follows: the dependency DAG stays acyclic and readiness stays in sync.
//! Starting, completing, and concluding tasks run the app's own commands.

use std::collections::{BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::commands::completion::{self, OpenPullRequest};
use crate::commands::herdr as herdr_commands;
use crate::commands::tasks::edit_task;
use crate::commands::types::{
    self, CommitRecord, Estimate, Task, TaskEdit, TaskId, TaskResolution, TaskStatus, TaskType,
    Workspace, WorkspaceId,
};
use crate::commands::workspaces;
use crate::commands::{CommandError, CommandErrorKind, CommandResult, StorageState, lock};
use crate::domain::model::{self, NoteKind};
use crate::storage::{self, Storage, StorageError, TaskSearch};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ListTasks {
    /// Only tasks of this workspace; all workspaces when omitted.
    pub workspace_id: Option<WorkspaceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CreateWorkspace {
    /// Name shown in Toodo, e.g. the repository name.
    pub name: String,
    /// Absolute path of the repository on the machine Toodo runs on: a Windows path such
    /// as `C:\dev\toodo` in the desktop app, a POSIX path such as `/home/me/toodo` on
    /// toodo-server. With `clone_url` it must not exist yet or be an empty directory;
    /// without it, it must be inside an existing clone, whose root becomes the workspace.
    pub path: String,
    /// Clone this repository URL into `path`; omit it to open the repository already
    /// cloned there.
    pub clone_url: Option<String>,
    /// The branch task pull requests go into, e.g. `dev`; detected from the repository's
    /// `origin` default branch when omitted.
    pub default_base_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CurrentTask {
    /// Absolute path of the agent's working directory, e.g. the worktree of its task.
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CreateTask {
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    /// Details in GitHub-flavored Markdown.
    #[serde(default)]
    pub description: String,
    /// Tasks the new task depends on, from any workspace.
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    /// Your estimate of the work, in hours, days, weeks, or months of active work.
    #[serde(default)]
    pub estimate: Option<Estimate>,
    /// The day the task should be done by, as YYYY-MM-DD, if the user gave one.
    #[serde(default)]
    pub due_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct UpdateTask {
    pub id: TaskId,
    /// Unchanged when omitted.
    pub task_type: Option<TaskType>,
    /// Unchanged when omitted.
    pub title: Option<String>,
    /// Details in GitHub-flavored Markdown; unchanged when omitted.
    pub description: Option<String>,
    /// Unchanged when omitted; null clears it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<Estimate>")]
    pub estimate: Option<Option<Estimate>>,
    /// YYYY-MM-DD; unchanged when omitted, null clears it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<String>")]
    pub due_date: Option<Option<String>>,
}

/// Tells a field given as `null` (`Some(None)`) from an omitted one (`None`, the default).
pub(super) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AddDependencies {
    /// The dependent task.
    pub task_id: TaskId,
    /// Its prerequisites, from any workspace, which must all be done before `task_id` is
    /// ready.
    pub depends_on: Vec<TaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Dependency {
    /// The dependent task.
    pub task_id: TaskId,
    /// The prerequisite task, which must be done before `task_id` is ready.
    pub depends_on: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AddNote {
    /// The task the note is about, usually your current task.
    pub task_id: TaskId,
    /// What happened, in GitHub-flavored Markdown.
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct TaskRef {
    /// The task. Never your own task, the one current_task finds.
    pub id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct CompleteTask {
    /// The task. Never your own task, the one current_task finds.
    pub id: TaskId,
    /// Only for a task whose work shipped with the merged pull request of another task,
    /// e.g. one pull request that fixed several tasks: that task's ID (`178` or `"#178"`),
    /// which must be done as merged, or the pull request URL. Omit it to complete the
    /// task with its own pull request.
    pub merged_with: Option<MergedWithRef>,
}

/// A task ID or a pull request URL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MergedWithRef {
    Task(i64),
    Text(String),
}

impl MergedWithRef {
    fn target(self) -> CommandResult<completion::MergedWithTarget> {
        let text = match self {
            Self::Task(id) => return Ok(completion::MergedWithTarget::Task(model::TaskId(id))),
            Self::Text(text) => text,
        };
        let text = text.trim();
        if let Ok(id) = text.strip_prefix('#').unwrap_or(text).parse::<i64>() {
            return Ok(completion::MergedWithTarget::Task(model::TaskId(id)));
        }
        if text.starts_with("https://") {
            return Ok(completion::MergedWithTarget::PullRequest(text.to_owned()));
        }
        Err(CommandError::invalid_input(format!(
            "merged_with must be a task ID such as 178 or a pull request URL, not {text:?}"
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchTasks {
    /// Words to look for in any language, e.g. `deploy server`; short words such as
    /// two-character Korean ones work too. Every word must appear in the
    /// task's title, description, notes, completion note, branch, or pull request, as a
    /// case-insensitive substring; words are taken literally, not as operators. Use a few
    /// distinctive words and search again with others to widen the search.
    pub query: String,
    /// Only tasks of this workspace; all workspaces when omitted.
    pub workspace_id: Option<WorkspaceId>,
    /// Only tasks with this status; any status when omitted.
    pub status: Option<TaskStatus>,
    /// At most this many results, best match first: 20 when omitted, at most 100.
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetTask {
    /// The task, e.g. one that search_tasks found.
    pub id: TaskId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct Related {
    /// One task of the link, e.g. your current task.
    pub task_id: TaskId,
    /// The other task, from any workspace. The link goes both ways.
    pub related_id: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ConcludeTask {
    /// The task. Never your own task, the one current_task finds.
    pub id: TaskId,
    /// The conclusion in GitHub-flavored Markdown: what was tried or found and why the
    /// work ends without a merge. Kept as the task's completion note.
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SetWorktreeRoot {
    /// SSH host of a remote machine, as in `machine.label` of list_machine_settings or a
    /// workspace's `location.machine`; omit it for the machine Toodo runs on.
    pub ssh_host: Option<String>,
    /// Absolute directory on that machine that holds the task worktrees, e.g.
    /// `/home/me/worktrees`: a Windows path such as `D:\worktrees` for the machine of the
    /// desktop app, a POSIX path on toodo-server and on SSH machines. Null or omitted
    /// clears it, so tasks on the machine cannot start until it is set again.
    pub worktree_root: Option<String>,
}

/// Settings of the machine Toodo runs on and of every known SSH machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineSettingsList {
    pub machines: Vec<types::MachineSettings>,
    /// Why the machines saved in herdr could not be read; the list then lacks those
    /// without a workspace or saved settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub herdr_error: Option<String>,
}

/// The local machine first, then the SSH machines saved in herdr, hosting a workspace, or
/// with saved settings, by label.
pub fn list_machine_settings(
    storage: &StorageState,
    herdr: &Herdr,
) -> CommandResult<MachineSettingsList> {
    let (saved, herdr_error) = match herdr.list_machines() {
        Ok(saved) => (saved, None),
        Err(error) => (Vec::new(), Some(CommandError::from(error).message)),
    };
    let storage = lock(storage);
    let mut hosts: BTreeSet<String> = saved
        .into_iter()
        .map(|machine| machine.target.trim().to_owned())
        .collect();
    for workspace in storage.list_workspaces()? {
        if let model::Machine::Ssh(label) = workspace.location.machine() {
            hosts.insert(label.as_str().to_owned());
        }
    }
    for label in storage.ssh_machines_with_settings()? {
        hosts.insert(label.as_str().to_owned());
    }
    let machines = std::iter::once(types::Machine::Local)
        .chain(hosts.into_iter().map(|label| types::Machine::Ssh { label }))
        .map(|machine| workspaces::machine_settings(&storage, machine))
        .collect::<CommandResult<_>>()?;
    Ok(MachineSettingsList {
        machines,
        herdr_error,
    })
}

/// Saves the worktree root of one machine through the app's own validation.
pub fn set_worktree_root(
    storage: &mut Storage,
    request: SetWorktreeRoot,
) -> CommandResult<types::MachineSettings> {
    let machine = match request.ssh_host {
        Some(label) => types::Machine::Ssh { label },
        None => types::Machine::Local,
    };
    workspaces::save_settings(
        storage,
        types::MachineSettings {
            machine,
            worktree_root: request.worktree_root,
        },
    )
}

pub fn list_workspaces(storage: &Storage) -> CommandResult<Vec<Workspace>> {
    Ok(types::map_all(&storage.list_workspaces()?))
}

/// Creates a local workspace through the app's own setup: clone or check the repository,
/// open it in herdr, and save it unless a workspace already uses its location.
pub fn create_workspace(
    storage: &StorageState,
    git: &Git,
    herdr: &Herdr,
    request: CreateWorkspace,
) -> CommandResult<Workspace> {
    let setup = types::NewWorkspace {
        name: request.name,
        location: types::WorkspaceLocation::Local { path: request.path },
        source: match request.clone_url {
            Some(url) => types::WorkspaceSource::Clone { url },
            None => types::WorkspaceSource::Open,
        },
        default_base_branch: request.default_base_branch,
    }
    .try_into()?;
    Ok((&workspaces::create(storage, git, herdr, setup)?).into())
}

pub fn list_tasks(storage: &Storage, request: ListTasks) -> CommandResult<Vec<Task>> {
    let tasks = match request.workspace_id {
        Some(workspace) => {
            storage
                .get_workspace(workspace.into())?
                .ok_or(StorageError::WorkspaceNotFound(workspace.into()))?;
            storage.list_tasks(workspace.into())?
        }
        None => storage.list_all_tasks()?,
    };
    Ok(types::map_all(&tasks))
}

/// The current task, with the base branch its pull request goes into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentTaskReply {
    #[serde(flatten)]
    pub task: Task,
    /// The default base branch of the task's workspace.
    pub base_branch: String,
    /// Whether you merge the task's pull request yourself: `never`, or `afterChecks`
    /// (into `base_branch` only, once every check on it passed).
    pub agent_merge: types::AgentMerge,
}

/// The task whose worktree contains `cwd`; the innermost one if worktrees are nested.
pub fn current_task(storage: &Storage, request: CurrentTask) -> CommandResult<CurrentTaskReply> {
    let cwd = comparable_path(&request.cwd);
    let task = storage
        .list_all_tasks()?
        .iter()
        .filter_map(|task| {
            let worktree = comparable_path(&task.worktree_path.as_ref()?.to_string());
            let inside = cwd == worktree
                || cwd
                    .strip_prefix(&worktree)
                    .is_some_and(|rest| rest.starts_with('/'));
            inside.then_some((worktree.len(), task))
        })
        .max_by_key(|(length, _)| *length)
        .map(|(_, task)| Task::from(task))
        .ok_or_else(|| {
            CommandError::not_found(format!(
                "no Toodo task has a worktree containing {}",
                request.cwd
            ))
        })?;
    let workspace = storage
        .get_workspace(task.workspace_id.into())?
        .ok_or(StorageError::WorkspaceNotFound(task.workspace_id.into()))?;
    Ok(CurrentTaskReply {
        task,
        base_branch: workspace.default_base_branch,
        agent_merge: workspace.agent_merge.into(),
    })
}

/// A path with `/` separators and no trailing separator; Windows paths, which are
/// case-insensitive, are also lowercased.
fn comparable_path(path: &str) -> String {
    let path = path.trim().replace('\\', "/");
    let path = path.trim_end_matches('/');
    let bytes = path.as_bytes();
    let windows = path.starts_with("//") || (bytes.len() >= 2 && bytes[1] == b':');
    if windows {
        path.to_lowercase()
    } else {
        path.to_owned()
    }
}

/// Creates a `planned` task with its dependencies. Unknown prerequisites are rejected
/// before anything is written; a new task has no dependents, so its edges cannot close
/// a cycle.
pub fn create_task(storage: &mut Storage, request: CreateTask) -> CommandResult<Task> {
    for &prerequisite in &request.depends_on {
        storage
            .get_task(prerequisite.into())?
            .ok_or(StorageError::TaskNotFound(prerequisite.into()))?;
    }
    let new: storage::NewTask = types::NewTask {
        workspace_id: request.workspace_id,
        task_type: request.task_type,
        title: request.title,
        description: request.description,
        estimate: request.estimate,
        due_date: request.due_date,
    }
    .try_into()?;
    let task = storage.create_task(&new)?;
    for &prerequisite in &request.depends_on {
        if let Err(error) = storage.add_dependency(task.id, prerequisite.into()) {
            if let Err(cleanup) = storage.delete_task(task.id) {
                eprintln!(
                    "toodo: failed to delete task {} after a failed MCP create: {cleanup}",
                    task.id
                );
            }
            return Err(error.into());
        }
    }
    get_task(storage, task.id.into())
}

pub fn update_task(storage: &mut Storage, request: UpdateTask) -> CommandResult<Task> {
    let current = get_task(storage, request.id)?;
    edit_task(
        storage,
        TaskEdit {
            id: request.id,
            task_type: request.task_type.unwrap_or(current.task_type),
            title: request.title.unwrap_or(current.title),
            description: request.description.unwrap_or(current.description),
            estimate: request.estimate.unwrap_or(current.estimate),
            due_date: request.due_date.unwrap_or(current.due_date),
        },
    )
}

/// Adds the edges in one step and returns the dependent task. Self and cyclic edges are
/// rejected, and one rejected edge adds none. An `in_progress` task that a prerequisite
/// blocks is paused; its herdr workspace stays open, and the app asks the user whether to
/// close it, since that ends the agent's session.
pub fn add_dependencies(storage: &mut Storage, request: AddDependencies) -> CommandResult<Task> {
    if request.depends_on.is_empty() {
        return Err(CommandError::invalid_input(
            "depends_on must name at least one task",
        ));
    }
    let depends_on: Vec<_> = request.depends_on.iter().map(|&id| id.into()).collect();
    let added = storage.add_dependencies(request.task_id.into(), &depends_on)?;
    Ok((&added.task).into())
}

pub fn remove_dependency(storage: &mut Storage, request: Dependency) -> CommandResult<Task> {
    storage.remove_dependency(request.task_id.into(), request.depends_on.into())?;
    get_task(storage, request.task_id)
}

/// Appends a note of `kind` to the task's history and returns the task with its notes.
pub fn add_note(storage: &mut Storage, kind: NoteKind, request: AddNote) -> CommandResult<Task> {
    let note = request.note.trim();
    if note.is_empty() {
        return Err(CommandError::invalid_input("note must not be empty"));
    }
    let task = storage.add_task_note(request.task_id.into(), kind, note)?;
    Ok((&task).into())
}

/// Starts or resumes a ready `planned` task as the app's Start does.
pub fn start_task(
    storage: &StorageState,
    git: &Git,
    herdr: &Herdr,
    request: TaskRef,
) -> CommandResult<Task> {
    let id = request.id.into();
    match herdr_commands::start(storage, git, herdr, id) {
        Ok(task) => Ok((&task).into()),
        Err(error) => Err(with_worktree_root_hint(&lock(storage), id, error)),
    }
}

/// Tells the agent how to fix a start rejected because the task's machine has no worktree
/// root; any other error is returned as it is.
fn with_worktree_root_hint(
    storage: &Storage,
    id: model::TaskId,
    mut error: CommandError,
) -> CommandError {
    if error.kind != CommandErrorKind::InvalidInput {
        return error;
    }
    let Ok(Some(task)) = storage.get_task(id) else {
        return error;
    };
    let Ok(Some(workspace)) = storage.get_workspace(task.workspace_id) else {
        return error;
    };
    let machine = workspace.location.machine();
    let root_missing = match storage.get_machine_settings(&machine) {
        Ok(model::MachineSettings::Local { worktree_root }) => worktree_root.is_none(),
        Ok(model::MachineSettings::Ssh { worktree_root, .. }) => worktree_root.is_none(),
        Err(_) => false,
    };
    if !root_missing {
        return error;
    }
    let host = match &machine {
        model::Machine::Local => String::new(),
        model::Machine::Ssh(label) => format!("ssh_host \"{label}\" and "),
    };
    error.message.push_str(&format!(
        "; ask the user where task worktrees go on that machine, call set_worktree_root \
         with {host}worktree_root set to that absolute directory, then start the task again"
    ));
    error
}

/// Completes an `in_review` task whose pull request is merged, as the app does, or with
/// `merged_with`, a task whose work shipped with another task's merged pull request.
pub fn complete_task(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    herdr: &Herdr,
    request: CompleteTask,
) -> CommandResult<Task> {
    let id = request.id.into();
    let task = match request.merged_with {
        None => completion::complete(storage, github, herdr, id)?,
        Some(merged_with) => completion::complete_merged_with(
            storage,
            git,
            github,
            herdr,
            id,
            merged_with.target()?,
        )?,
    };
    Ok((&task).into())
}

/// Concludes a task without merging as the app does, except that a task whose pull
/// request is still open is rejected instead of closing it.
pub fn conclude_task(
    storage: &StorageState,
    git: &Git,
    github: &GitHub,
    herdr: &Herdr,
    request: ConcludeTask,
) -> CommandResult<Task> {
    Ok((&completion::conclude(
        storage,
        git,
        github,
        herdr,
        request.id.into(),
        &request.note,
        OpenPullRequest::Reject,
    )?)
        .into())
}

const DEFAULT_SEARCH_LIMIT: u32 = 20;
const MAX_SEARCH_LIMIT: u32 = 100;

/// A task that search_tasks found, with its links to other tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    /// The name of the task's workspace.
    pub workspace: String,
    pub task_type: TaskType,
    pub title: String,
    pub status: TaskStatus,
    pub resolution: Option<TaskResolution>,
    pub pull_request_url: Option<String>,
    /// Tasks this task waits on.
    pub depends_on: Vec<TaskId>,
    /// Tasks that wait on this task.
    pub dependents: Vec<TaskId>,
    /// Related tasks, which neither wait on this task nor block it.
    pub related: Vec<TaskId>,
}

/// Another task linked to a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLink {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub title: String,
    pub status: TaskStatus,
    pub resolution: Option<TaskResolution>,
}

/// A task with its whole history and its links, as get_task returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetails {
    /// The task: description, status timeline, notes, completion note (the conclusion
    /// of a task done as not_adopted), and pull request.
    pub task: Task,
    /// The name of the task's workspace.
    pub workspace: String,
    /// The commits recorded for the task, from its branch or its merged pull request.
    pub commits: Vec<CommitRecord>,
    /// Tasks this task waits on.
    pub depends_on: Vec<TaskLink>,
    /// Tasks that wait on this task.
    pub dependents: Vec<TaskLink>,
    /// Related tasks, which neither wait on this task nor block it.
    pub related: Vec<TaskLink>,
}

/// The links between all tasks: what each task waits on, what waits on it, and what
/// is related to it.
struct Links {
    tasks: HashMap<model::TaskId, model::Task>,
    workspaces: HashMap<model::WorkspaceId, String>,
    dependents: HashMap<model::TaskId, BTreeSet<model::TaskId>>,
    related: HashMap<model::TaskId, BTreeSet<model::TaskId>>,
}

impl Links {
    fn load(storage: &Storage) -> CommandResult<Self> {
        let tasks: HashMap<_, _> = storage
            .list_all_tasks()?
            .into_iter()
            .map(|task| (task.id, task))
            .collect();
        let mut dependents: HashMap<_, BTreeSet<_>> = HashMap::new();
        for task in tasks.values() {
            for &prerequisite in &task.depends_on {
                dependents.entry(prerequisite).or_default().insert(task.id);
            }
        }
        let mut related: HashMap<_, BTreeSet<_>> = HashMap::new();
        for (a, b) in storage.list_relations()? {
            related.entry(a).or_default().insert(b);
            related.entry(b).or_default().insert(a);
        }
        let workspaces = storage
            .list_workspaces()?
            .into_iter()
            .map(|workspace| (workspace.id, workspace.name))
            .collect();
        Ok(Self {
            tasks,
            workspaces,
            dependents,
            related,
        })
    }

    fn task(&self, id: model::TaskId) -> CommandResult<&model::Task> {
        Ok(self.tasks.get(&id).ok_or(StorageError::TaskNotFound(id))?)
    }

    fn workspace(&self, task: &model::Task) -> String {
        self.workspaces
            .get(&task.workspace_id)
            .cloned()
            .unwrap_or_default()
    }

    fn dependents(&self, id: model::TaskId) -> impl Iterator<Item = model::TaskId> + '_ {
        self.dependents.get(&id).into_iter().flatten().copied()
    }

    fn related(&self, id: model::TaskId) -> impl Iterator<Item = model::TaskId> + '_ {
        self.related.get(&id).into_iter().flatten().copied()
    }

    fn hit(&self, id: model::TaskId) -> CommandResult<SearchHit> {
        let task = self.task(id)?;
        Ok(SearchHit {
            id: id.into(),
            workspace_id: task.workspace_id.into(),
            workspace: self.workspace(task),
            task_type: task.task_type.into(),
            title: task.title.clone(),
            status: task.status.into(),
            resolution: task.resolution.map(TaskResolution::from),
            pull_request_url: task.pull_request_url.clone(),
            depends_on: task.depends_on.iter().copied().map(TaskId::from).collect(),
            dependents: self.dependents(id).map(TaskId::from).collect(),
            related: self.related(id).map(TaskId::from).collect(),
        })
    }

    fn links(&self, ids: impl Iterator<Item = model::TaskId>) -> Vec<TaskLink> {
        ids.filter_map(|id| self.tasks.get(&id))
            .map(|task| TaskLink {
                id: task.id.into(),
                workspace_id: task.workspace_id.into(),
                title: task.title.clone(),
                status: task.status.into(),
                resolution: task.resolution.map(TaskResolution::from),
            })
            .collect()
    }
}

/// Tasks matching the query, best match first, each with its links.
pub fn search_tasks(storage: &Storage, request: SearchTasks) -> CommandResult<Vec<SearchHit>> {
    if request.query.trim().is_empty() {
        return Err(CommandError::invalid_input("query must contain a word"));
    }
    if let Some(workspace) = request.workspace_id {
        storage
            .get_workspace(workspace.into())?
            .ok_or(StorageError::WorkspaceNotFound(workspace.into()))?;
    }
    let ids = storage.search_tasks(&TaskSearch {
        query: &request.query,
        workspace: request.workspace_id.map(Into::into),
        status: request.status.map(Into::into),
        limit: request
            .limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT),
    })?;
    let links = Links::load(storage)?;
    ids.into_iter().map(|id| links.hit(id)).collect()
}

/// A task with its history, commits, and the tasks it is linked to.
pub fn task_details(storage: &Storage, request: GetTask) -> CommandResult<TaskDetails> {
    let links = Links::load(storage)?;
    let id = request.id.into();
    let task = links.task(id)?;
    Ok(TaskDetails {
        task: task.into(),
        workspace: links.workspace(task),
        commits: types::map_all(&storage.list_commit_records(id)?),
        depends_on: links.links(task.depends_on.iter().copied()),
        dependents: links.links(links.dependents(id)),
        related: links.links(links.related(id)),
    })
}

/// Links two tasks as related and returns the first with its links.
pub fn add_related(storage: &mut Storage, request: Related) -> CommandResult<TaskDetails> {
    storage.add_related(request.task_id.into(), request.related_id.into())?;
    task_details(
        storage,
        GetTask {
            id: request.task_id,
        },
    )
}

/// Removes the related link between two tasks and returns the first with its links.
pub fn remove_related(storage: &mut Storage, request: Related) -> CommandResult<TaskDetails> {
    storage.remove_related(request.task_id.into(), request.related_id.into())?;
    task_details(
        storage,
        GetTask {
            id: request.task_id,
        },
    )
}

fn get_task(storage: &Storage, id: TaskId) -> CommandResult<Task> {
    let task = storage
        .get_task(id.into())?
        .ok_or(StorageError::TaskNotFound(id.into()))?;
    Ok((&task).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;
    use crate::domain::model::{self, HostPath, WorkspaceLocation};
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::storage::NewWorkspace;

    fn storage_with_workspace() -> (Storage, WorkspaceId) {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "toodo".to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        (storage, workspace.id.into())
    }

    fn create(
        storage: &mut Storage,
        workspace: WorkspaceId,
        title: &str,
        depends_on: Vec<TaskId>,
    ) -> CommandResult<Task> {
        create_task(
            storage,
            CreateTask {
                workspace_id: workspace,
                task_type: TaskType::Feat,
                title: title.to_owned(),
                description: String::new(),
                depends_on,
                estimate: None,
                due_date: None,
            },
        )
    }

    fn set_worktree(storage: &mut Storage, id: TaskId, path: HostPath) {
        let mut task = storage.get_task(id.into()).unwrap().unwrap();
        task.worktree_path = Some(path);
        storage.update_task(&task).unwrap();
    }

    #[test]
    fn creates_a_task_with_its_dependencies() {
        let (mut storage, workspace) = storage_with_workspace();
        let first = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let second = create(&mut storage, workspace, "API", vec![first.id]).unwrap();
        assert_eq!(second.depends_on, vec![first.id]);
        assert_eq!(second.status, types::TaskStatus::Planned);
        let stored = storage.get_task(second.id.into()).unwrap().unwrap();
        assert_eq!(Task::from(&stored), second);
    }

    #[test]
    fn an_unknown_prerequisite_or_empty_title_creates_nothing() {
        let (mut storage, workspace) = storage_with_workspace();
        let error = create(&mut storage, workspace, "API", vec![TaskId(42)]).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        let error = create(&mut storage, workspace, "  ", vec![]).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        assert!(storage.list_all_tasks().unwrap().is_empty());
    }

    #[test]
    fn a_dependency_that_closes_a_cycle_is_rejected() {
        let (mut storage, workspace) = storage_with_workspace();
        let first = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let second = create(&mut storage, workspace, "API", vec![first.id]).unwrap();
        let request = AddDependencies {
            task_id: first.id,
            depends_on: vec![second.id],
        };
        let error = add_dependencies(&mut storage, request.clone()).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("cycle"), "{}", error.message);
        assert!(get_task(&storage, first.id).unwrap().depends_on.is_empty());

        let removed = remove_dependency(
            &mut storage,
            Dependency {
                task_id: second.id,
                depends_on: first.id,
            },
        )
        .unwrap();
        assert!(removed.depends_on.is_empty());
        assert_eq!(
            add_dependencies(&mut storage, request).unwrap().depends_on,
            vec![second.id]
        );
    }

    #[test]
    fn adds_several_prerequisites_at_once_and_needs_at_least_one() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Verify", vec![]).unwrap();
        let fix = create(&mut storage, workspace, "Fix login", vec![]).unwrap();
        let docs = create(&mut storage, workspace, "Fix docs", vec![]).unwrap();

        let error = add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![],
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let updated = add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![fix.id, docs.id],
            },
        )
        .unwrap();
        assert_eq!(updated.depends_on, vec![fix.id, docs.id]);
    }

    #[test]
    fn a_blocking_prerequisite_pauses_the_task_but_leaves_its_herdr_workspace_open() {
        use crate::domain::status::TaskStatus;
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Verify", vec![]).unwrap();
        let fix = create(&mut storage, workspace, "Fix login", vec![]).unwrap();
        storage
            .set_task_status(task.id.into(), TaskStatus::InProgress)
            .unwrap();
        storage
            .set_task_herdr_workspace(task.id.into(), "w1")
            .unwrap();

        add_dependencies(
            &mut storage,
            AddDependencies {
                task_id: task.id,
                depends_on: vec![fix.id],
            },
        )
        .unwrap();

        let paused = storage.get_task(task.id.into()).unwrap().unwrap();
        assert_eq!(paused.status, TaskStatus::Planned);
        assert_eq!(paused.herdr_workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn update_changes_only_the_given_fields() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let updated = update_task(
            &mut storage,
            UpdateTask {
                id: task.id,
                task_type: None,
                title: None,
                description: Some("Add tables".to_owned()),
                estimate: None,
                due_date: None,
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Schema");
        assert_eq!(updated.task_type, TaskType::Feat);
        assert_eq!(updated.description, "Add tables");
    }

    #[test]
    fn estimates_and_due_dates_are_set_kept_when_omitted_and_cleared_by_null() {
        let (mut storage, workspace) = storage_with_workspace();
        let request: CreateTask = serde_json::from_value(serde_json::json!({
            "workspace_id": workspace,
            "task_type": "feat",
            "title": "Schema",
            "estimate": { "unit": "hours", "amount": 2.5 },
            "due_date": "2026-10-20",
        }))
        .unwrap();
        let task = create_task(&mut storage, request).unwrap();
        let planned = Some(Estimate {
            unit: types::EstimateUnit::Hours,
            amount: 2.5,
        });
        assert_eq!(task.estimate, planned);
        assert_eq!(task.due_date.as_deref(), Some("2026-10-20"));

        let update = |storage: &mut Storage, fields: serde_json::Value| {
            let mut request = serde_json::json!({ "id": task.id });
            request
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            update_task(storage, serde_json::from_value(request).unwrap())
        };
        let kept = update(&mut storage, serde_json::json!({ "title": "Tables" })).unwrap();
        assert_eq!(
            (kept.estimate, kept.due_date.as_deref()),
            (planned, Some("2026-10-20"))
        );
        let weeks = update(
            &mut storage,
            serde_json::json!({ "estimate": { "unit": "weeks", "amount": 3 }, "due_date": null }),
        )
        .unwrap();
        assert_eq!(
            (weeks.estimate, weeks.due_date),
            (
                Some(Estimate {
                    unit: types::EstimateUnit::Weeks,
                    amount: 3.0
                }),
                None
            )
        );
        let cleared = update(&mut storage, serde_json::json!({ "estimate": null })).unwrap();
        assert_eq!(cleared.estimate, None);
        assert_eq!(
            Task::from(&storage.get_task(task.id.into()).unwrap().unwrap()),
            cleared
        );

        for invalid in [
            serde_json::json!({ "due_date": "2026-02-30" }),
            serde_json::json!({ "estimate": { "unit": "hours", "amount": 0 } }),
        ] {
            let error = update(&mut storage, invalid.clone()).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::InvalidInput, "{invalid}");
        }
        assert_eq!(
            Task::from(&storage.get_task(task.id.into()).unwrap().unwrap()),
            cleared
        );
    }

    #[test]
    fn notes_are_trimmed_and_a_blank_note_or_unknown_task_adds_none() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let note = |task_id: TaskId, note: &str| AddNote {
            task_id,
            note: note.to_owned(),
        };

        let noted = add_note(
            &mut storage,
            NoteKind::Progress,
            note(task.id, "  Chose SQLite \n"),
        )
        .unwrap();
        let noted = add_note(&mut storage, NoteKind::Blocker, note(noted.id, "No access")).unwrap();
        let notes: Vec<_> = noted
            .notes
            .iter()
            .map(|note| (note.kind, note.body.as_str()))
            .collect();
        assert_eq!(
            notes,
            [
                (types::NoteKind::Progress, "Chose SQLite"),
                (types::NoteKind::Blocker, "No access"),
            ]
        );

        let error = add_note(&mut storage, NoteKind::Blocker, note(task.id, " \n")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        let error = add_note(&mut storage, NoteKind::Progress, note(TaskId(99), "x")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert_eq!(get_task(&storage, task.id).unwrap().notes.len(), 2);
    }

    fn search(storage: &Storage, query: &str) -> CommandResult<Vec<SearchHit>> {
        search_tasks(
            storage,
            SearchTasks {
                query: query.to_owned(),
                workspace_id: None,
                status: None,
                limit: None,
            },
        )
    }

    #[test]
    fn search_hits_and_task_details_carry_history_and_links() {
        let (mut storage, workspace) = storage_with_workspace();
        let research = create(&mut storage, workspace, "Research search engines", vec![]).unwrap();
        let schema = create(&mut storage, workspace, "Search schema", vec![]).unwrap();
        let tool = create(&mut storage, workspace, "Search tool", vec![schema.id]).unwrap();
        add_related(
            &mut storage,
            Related {
                task_id: tool.id,
                related_id: research.id,
            },
        )
        .unwrap();
        add_note(
            &mut storage,
            NoteKind::Progress,
            AddNote {
                task_id: research.id,
                note: "Chose FTS5 over tantivy".to_owned(),
            },
        )
        .unwrap();
        storage
            .set_task_status(
                research.id.into(),
                crate::domain::status::TaskStatus::InProgress,
            )
            .unwrap();
        storage
            .conclude_task(research.id.into(), "Too heavy for a desktop app", &[], None)
            .unwrap();

        let hits = search(&storage, "search").unwrap();
        let ids: Vec<_> = hits.iter().map(|hit| hit.id).collect();
        assert_eq!(ids.len(), 3);
        let hit = hits.iter().find(|hit| hit.id == schema.id).unwrap();
        assert_eq!(
            (hit.workspace.as_str(), &hit.dependents, &hit.related),
            ("toodo", &vec![tool.id], &vec![])
        );
        let hit = &search(&storage, "tantivy").unwrap()[0];
        assert_eq!(hit.id, research.id);
        assert_eq!(hit.related, [tool.id]);
        assert_eq!(hit.resolution, Some(TaskResolution::NotAdopted));
        assert_eq!(
            search(&storage, " ").unwrap_err().kind,
            CommandErrorKind::InvalidInput
        );

        let details = task_details(&storage, GetTask { id: research.id }).unwrap();
        assert_eq!(
            details.task.completion_note.as_deref(),
            Some("Too heavy for a desktop app")
        );
        assert_eq!(details.task.notes[0].body, "Chose FTS5 over tantivy");
        let statuses: Vec<_> = details
            .task
            .timeline
            .iter()
            .map(|event| event.status)
            .collect();
        assert_eq!(
            statuses,
            [
                TaskStatus::Planned,
                TaskStatus::InProgress,
                TaskStatus::Done
            ]
        );
        assert_eq!(
            details.related,
            [TaskLink {
                id: tool.id,
                workspace_id: workspace,
                title: "Search tool".to_owned(),
                status: TaskStatus::Planned,
                resolution: None,
            }]
        );
        let details = task_details(&storage, GetTask { id: tool.id }).unwrap();
        let titles = |links: &[TaskLink]| -> Vec<String> {
            links.iter().map(|link| link.title.clone()).collect()
        };
        assert_eq!(titles(&details.depends_on), ["Search schema"]);
        assert!(details.dependents.is_empty());
        assert_eq!(titles(&details.related), ["Research search engines"]);
        let json = serde_json::to_value(&details).unwrap();
        assert_eq!(json["dependsOn"][0]["status"], "planned");
        assert_eq!(json["task"]["id"], serde_json::json!(tool.id));

        let unlinked = remove_related(
            &mut storage,
            Related {
                task_id: research.id,
                related_id: tool.id,
            },
        )
        .unwrap();
        assert!(unlinked.related.is_empty());
        let error = task_details(&storage, GetTask { id: TaskId(99) }).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        let error = add_related(
            &mut storage,
            Related {
                task_id: tool.id,
                related_id: tool.id,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
    }

    #[test]
    fn lists_tasks_of_one_workspace_or_all() {
        let (mut storage, workspace) = storage_with_workspace();
        create(&mut storage, workspace, "Schema", vec![]).unwrap();
        let all = list_tasks(&storage, ListTasks { workspace_id: None }).unwrap();
        assert_eq!(all.len(), 1);
        let error = list_tasks(
            &storage,
            ListTasks {
                workspace_id: Some(WorkspaceId(99)),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
    }

    #[test]
    fn the_current_task_is_the_one_whose_worktree_contains_the_cwd() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        create(&mut storage, workspace, "Other", vec![]).unwrap();
        set_worktree(
            &mut storage,
            task.id,
            HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-schema-1").unwrap()),
        );
        let find = |cwd: &str| {
            current_task(
                &storage,
                CurrentTask {
                    cwd: cwd.to_owned(),
                },
            )
        };

        let found = find(r"D:\wt\toodo\feat-schema-1").unwrap();
        assert_eq!(found.task.id, task.id);
        assert_eq!(found.base_branch, "main");
        let json = serde_json::to_value(&found).unwrap();
        assert_eq!(json["baseBranch"], "main");
        assert_eq!(json["id"], serde_json::json!(task.id));
        assert_eq!(
            find("d:/WT/toodo/feat-schema-1/src/").unwrap().task.id,
            task.id
        );
        let error = find(r"D:\wt\toodo\feat-schema-10").unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::NotFound);
        assert!(find(r"D:\wt\toodo").is_err());
    }

    #[test]
    fn the_current_task_tells_whether_the_agent_merges_its_pull_request() {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Schema", vec![]).unwrap();
        set_worktree(
            &mut storage,
            task.id,
            HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-schema-1").unwrap()),
        );
        let agent_merge = |storage: &Storage| {
            let found = current_task(
                storage,
                CurrentTask {
                    cwd: r"D:\wt\toodo\feat-schema-1".to_owned(),
                },
            )
            .unwrap();
            serde_json::to_value(&found).unwrap()["agentMerge"].clone()
        };
        assert_eq!(agent_merge(&storage), "never");

        let mut saved = storage.get_workspace(workspace.into()).unwrap().unwrap();
        saved.agent_merge = model::AgentMerge::AfterChecks;
        storage.update_workspace(&saved).unwrap();
        assert_eq!(agent_merge(&storage), "afterChecks");
    }

    #[test]
    fn posix_worktrees_are_matched_case_sensitively() {
        let mut storage = Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&NewWorkspace {
                name: "server".to_owned(),
                location: WorkspaceLocation::Ssh {
                    machine: model::MachineLabel::parse("devbox").unwrap(),
                    path: PosixPath::parse("/srv/app").unwrap(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let task = create(&mut storage, workspace.id.into(), "Remote", vec![]).unwrap();
        set_worktree(
            &mut storage,
            task.id,
            HostPath::Posix(PosixPath::parse("/wt/app/feat-remote-1").unwrap()),
        );
        let find = |cwd: &str| {
            current_task(
                &storage,
                CurrentTask {
                    cwd: cwd.to_owned(),
                },
            )
        };
        assert_eq!(find("/wt/app/feat-remote-1/src").unwrap().task.id, task.id);
        assert!(find("/WT/app/feat-remote-1").is_err());
    }

    type Scripted = Vec<Result<String, crate::adapters::git::GitError>>;

    /// An adapter whose runner returns `results` in order, and the runner.
    fn fake<T>(
        new: impl FnOnce(Box<dyn crate::adapters::git::Runner>) -> T,
        results: Scripted,
    ) -> (T, std::sync::Arc<crate::adapters::git::fake::FakeRunner>) {
        let runner = std::sync::Arc::new(crate::adapters::git::fake::FakeRunner::new(results));
        (new(Box::new(runner.clone())), runner)
    }

    const PR: &str = "https://github.com/acme/toodo/pull/12";

    /// `gh pr view` output for the pull request in `state`.
    fn viewed(state: &str) -> Result<String, crate::adapters::git::GitError> {
        let merged_at = if state == "MERGED" {
            r#""2026-10-09T14:47:45Z""#
        } else {
            "null"
        };
        Ok(format!(
            r#"{{"baseRefName":"main","mergedAt":{merged_at},"state":"{state}","url":"{PR}","statusCheckRollup":[],"reviewDecision":"","latestReviews":[]}}"#
        ))
    }

    /// `gh pr list` output with the pull request in `state`.
    fn listed(state: &str) -> Result<String, crate::adapters::git::GitError> {
        let merged_at = if state == "MERGED" {
            r#""2026-10-09T14:47:45Z""#
        } else {
            "null"
        };
        Ok(format!(
            r#"[{{"baseRefName":"main","mergedAt":{merged_at},"state":"{state}","url":"{PR}"}}]"#
        ))
    }

    /// Git output for a branch log with no commits and an `origin` on GitHub.
    fn github_checkout() -> Scripted {
        vec![
            Ok(String::new()),
            Ok("origin\n".to_owned()),
            Ok("git@github.com:acme/toodo.git\n".to_owned()),
        ]
    }

    const MERGED_DETAILS: &str = r#"{"additions":1,"author":{"login":"ada"},"changedFiles":1,"commits":[],"deletions":0,"mergeCommit":{"oid":"2222222222222222222222222222222222222222"},"number":12,"reviews":[],"title":"Add login"}"#;

    const CLOSED: &str = r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#;

    /// A task started in herdr workspace `w27`, in review with a pull request if
    /// `in_review`, and a planned task that depends on it.
    fn started(in_review: bool) -> (StorageState, Task, Task) {
        let (mut storage, workspace) = storage_with_workspace();
        let task = create(&mut storage, workspace, "Add login", vec![]).unwrap();
        let dependent = create(&mut storage, workspace, "Use login", vec![task.id]).unwrap();
        let checkout = crate::domain::naming::TaskCheckout {
            path: HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-add-login-1").unwrap()),
            branch: "feat/add-login-1".to_owned(),
        };
        let mut started = storage
            .start_task(task.id.into(), &checkout, "w27")
            .unwrap();
        if in_review {
            started = storage
                .open_pull_request(task.id.into(), &[], PR, None)
                .unwrap();
        }
        (std::sync::Mutex::new(storage), (&started).into(), dependent)
    }

    fn stored(storage: &StorageState, id: TaskId) -> Task {
        get_task(&crate::commands::lock(storage), id).unwrap()
    }

    fn last_status(task: &Task) -> Option<types::TaskStatus> {
        task.timeline.last().map(|event| event.status)
    }

    #[test]
    fn start_is_rejected_for_a_task_that_is_not_ready_or_not_planned() {
        let (storage, task, dependent) = started(false);
        let (git, git_runner) = fake(Git::new, vec![]);
        let (herdr, herdr_runner) = fake(Herdr::new, vec![]);

        let error = start_task(&storage, &git, &herdr, TaskRef { id: dependent.id }).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains("dependencies are done"),
            "{}",
            error.message
        );
        let error = start_task(&storage, &git, &herdr, TaskRef { id: task.id }).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);

        assert!(git_runner.invocations.lock().unwrap().is_empty());
        assert!(herdr_runner.invocations.lock().unwrap().is_empty());
        assert_eq!(stored(&storage, dependent.id), dependent);
    }

    #[test]
    fn complete_finishes_a_task_whose_pull_request_is_merged_and_only_that() {
        let (storage, task, _) = started(true);
        let (no_git, _) = fake(Git::new, vec![]);
        let (open, _) = fake(GitHub::new, vec![viewed("OPEN")]);
        let (no_herdr, _) = fake(Herdr::new, vec![]);

        let error = complete_task(&storage, &no_git, &open, &no_herdr, own(task.id)).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains("is not merged yet"),
            "{}",
            error.message
        );
        assert_eq!(stored(&storage, task.id), task);

        let (merged, _) = fake(
            GitHub::new,
            vec![viewed("MERGED"), Ok(MERGED_DETAILS.to_owned())],
        );
        let (herdr, herdr_runner) = fake(Herdr::new, vec![Ok(CLOSED.to_owned())]);
        let done = complete_task(&storage, &no_git, &merged, &herdr, own(task.id)).unwrap();
        assert_eq!(done.status, types::TaskStatus::Done);
        assert_eq!(done.resolution, Some(types::TaskResolution::Merged));
        assert_eq!(last_status(&done), Some(types::TaskStatus::Done));
        assert_eq!(done.herdr_workspace_id, None);
        assert_eq!(stored(&storage, task.id), done);
        assert_eq!(
            herdr_runner.invocations.lock().unwrap()[0].args,
            ["workspace", "close", "w27"]
        );
    }

    #[test]
    fn complete_is_rejected_for_a_task_that_is_not_in_review() {
        let (storage, task, _) = started(false);
        let (git, _) = fake(Git::new, vec![]);
        let (github, gh_runner) = fake(GitHub::new, vec![]);
        let (herdr, _) = fake(Herdr::new, vec![]);

        let error = complete_task(&storage, &git, &github, &herdr, own(task.id)).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains("only an in_review task"),
            "{}",
            error.message
        );
        assert!(gh_runner.invocations.lock().unwrap().is_empty());
        assert_eq!(stored(&storage, task.id), task);
    }

    fn own(id: TaskId) -> CompleteTask {
        CompleteTask {
            id,
            merged_with: None,
        }
    }

    fn merged_with(id: TaskId, target: MergedWithRef) -> CompleteTask {
        CompleteTask {
            id,
            merged_with: Some(target),
        }
    }

    /// `started(true)` with its task completed by the merge of its pull request, and an
    /// extra task of the same workspace started in herdr workspace `w28` whose work that
    /// pull request shipped too.
    fn shipped() -> (StorageState, Task, Task) {
        let (storage, task, _) = started(true);
        let extra = {
            let mut storage = crate::commands::lock(&storage);
            storage
                .complete_task(task.id.into(), Some(1_760_021_265), None, &[])
                .unwrap();
            let extra = create(&mut storage, task.workspace_id, "Fix typo", vec![]).unwrap();
            let checkout = crate::domain::naming::TaskCheckout {
                path: HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\fix-typo-3").unwrap()),
                branch: "fix/typo-3".to_owned(),
            };
            (&storage
                .start_task(extra.id.into(), &checkout, "w28")
                .unwrap())
                .into()
        };
        let task = stored(&storage, task.id);
        (storage, task, extra)
    }

    #[test]
    fn complete_with_merged_with_ships_a_task_through_another_tasks_pull_request() {
        let (storage, task, extra) = shipped();
        let (git, _) = fake(Git::new, github_checkout());
        let (github, gh_runner) = fake(GitHub::new, vec![Ok("[]".to_owned())]);
        let (herdr, herdr_runner) = fake(Herdr::new, vec![Ok(CLOSED.to_owned())]);

        let done = complete_task(
            &storage,
            &git,
            &github,
            &herdr,
            merged_with(extra.id, MergedWithRef::Text(format!(" #{} ", task.id.0))),
        )
        .unwrap();

        assert_eq!(done.status, types::TaskStatus::Done);
        assert_eq!(done.resolution, Some(types::TaskResolution::Merged));
        assert_eq!(
            done.merged_with,
            Some(types::MergedWith {
                task_id: Some(task.id),
                pull_request_url: PR.to_owned(),
            })
        );
        assert_eq!(done.pull_request_merged_at, task.pull_request_merged_at);
        assert_eq!(done.pull_request_url, None);
        assert_eq!(last_status(&done), Some(types::TaskStatus::Done));
        assert_eq!(done.herdr_workspace_id, None);
        assert_eq!(stored(&storage, extra.id), done);
        // Only the pull requests of its own branch were looked up.
        assert_eq!(gh_runner.invocations.lock().unwrap().len(), 1);
        assert_eq!(
            herdr_runner.invocations.lock().unwrap()[0].args,
            ["workspace", "close", "w28"]
        );
    }

    #[test]
    fn complete_with_merged_with_changes_a_task_concluded_as_not_adopted() {
        let (storage, task, extra) = shipped();
        let concluded: Task = (&crate::commands::lock(&storage)
            .conclude_task(
                extra.id.into(),
                "Fixed by the login pull request",
                &[],
                None,
            )
            .unwrap())
            .into();
        let (no_git, git_runner) = fake(Git::new, vec![]);
        let (github, _) = fake(GitHub::new, vec![viewed("MERGED")]);
        let (no_herdr, herdr_runner) = fake(Herdr::new, vec![]);

        let done = complete_task(
            &storage,
            &no_git,
            &github,
            &no_herdr,
            merged_with(extra.id, MergedWithRef::Text(PR.to_owned())),
        )
        .unwrap();

        assert_eq!(done.resolution, Some(types::TaskResolution::Merged));
        // The pull request URL finds the task that owns it.
        assert_eq!(
            done.merged_with,
            Some(types::MergedWith {
                task_id: Some(task.id),
                pull_request_url: PR.to_owned(),
            })
        );
        assert_eq!(done.timeline, concluded.timeline);
        assert_eq!(done.completion_note, concluded.completion_note);
        assert_eq!(stored(&storage, extra.id), done);
        assert!(git_runner.invocations.lock().unwrap().is_empty());
        assert!(herdr_runner.invocations.lock().unwrap().is_empty());

        // Completing it again, either way, has nothing left to do.
        let (no_github, _) = fake(GitHub::new, vec![]);
        let error = complete_task(
            &storage,
            &no_git,
            &no_github,
            &no_herdr,
            merged_with(extra.id, MergedWithRef::Task(task.id.0)),
        )
        .unwrap_err();
        assert!(
            error.message.contains("already done as merged"),
            "{}",
            error.message
        );
    }

    #[test]
    fn complete_of_a_not_adopted_task_without_merged_with_suggests_it() {
        let (storage, _, extra) = shipped();
        crate::commands::lock(&storage)
            .conclude_task(extra.id.into(), "Fixed elsewhere", &[], None)
            .unwrap();
        let (git, _) = fake(Git::new, vec![]);
        let (github, _) = fake(GitHub::new, vec![]);
        let (herdr, _) = fake(Herdr::new, vec![]);

        let error = complete_task(&storage, &git, &github, &herdr, own(extra.id)).unwrap_err();

        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("merged_with"), "{}", error.message);
    }

    #[test]
    fn complete_with_merged_with_is_rejected_until_the_work_shipped() {
        let (storage, in_review, dependent) = started(true);
        let (extra, not_adopted) = {
            let mut storage = crate::commands::lock(&storage);
            let workspace = in_review.workspace_id;
            let extra = create(&mut storage, workspace, "Fix typo", vec![]).unwrap();
            let other = create(&mut storage, workspace, "Try a cache", vec![]).unwrap();
            let checkout = crate::domain::naming::TaskCheckout {
                path: HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\rnd-cache-4").unwrap()),
                branch: "rnd/cache-4".to_owned(),
            };
            storage
                .start_task(other.id.into(), &checkout, "w29")
                .unwrap();
            storage
                .conclude_task(other.id.into(), "Too slow", &[], None)
                .unwrap();
            let checkout = crate::domain::naming::TaskCheckout {
                path: HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\fix-typo-3").unwrap()),
                branch: "fix/typo-3".to_owned(),
            };
            let extra: Task = (&storage
                .start_task(extra.id.into(), &checkout, "w28")
                .unwrap())
                .into();
            (extra, other)
        };
        let (no_git, _) = fake(Git::new, vec![]);
        let (no_herdr, herdr_runner) = fake(Herdr::new, vec![]);
        let reject = |id: TaskId, target: MergedWithRef, github: Scripted, reason: &str| {
            let (github, _) = fake(GitHub::new, github);
            let error = complete_task(
                &storage,
                &no_git,
                &github,
                &no_herdr,
                merged_with(id, target),
            )
            .unwrap_err();
            assert!(
                error.message.contains(reason),
                "{reason}: {}",
                error.message
            );
        };

        reject(extra.id, MergedWithRef::Task(extra.id.0), vec![], "itself");
        reject(
            extra.id,
            MergedWithRef::Task(in_review.id.0),
            vec![],
            "is in_review",
        );
        reject(
            extra.id,
            MergedWithRef::Task(not_adopted.id.0),
            vec![],
            "not adopted",
        );
        reject(
            extra.id,
            MergedWithRef::Task(dependent.id.0),
            vec![],
            "is planned",
        );
        reject(extra.id, MergedWithRef::Task(999), vec![], "999");
        reject(
            extra.id,
            MergedWithRef::Text("soon".to_owned()),
            vec![],
            "task ID",
        );
        reject(
            extra.id,
            MergedWithRef::Text(PR.to_owned()),
            vec![viewed("OPEN")],
            "not merged yet",
        );
        reject(
            extra.id,
            MergedWithRef::Text(PR.to_owned()),
            vec![viewed("CLOSED")],
            "shipped nothing",
        );
        reject(
            in_review.id,
            MergedWithRef::Text(PR.to_owned()),
            vec![],
            "task's own",
        );
        reject(
            dependent.id,
            MergedWithRef::Task(in_review.id.0),
            vec![],
            "only an in_progress",
        );

        assert!(herdr_runner.invocations.lock().unwrap().is_empty());
        assert_eq!(stored(&storage, extra.id), extra);
        assert_eq!(stored(&storage, in_review.id), in_review);
    }

    #[test]
    fn complete_with_merged_with_is_rejected_while_its_own_branch_has_a_pull_request() {
        let (storage, task, extra) = shipped();
        for (state, reason) in [("OPEN", "is still open"), ("MERGED", "was merged")] {
            let (git, _) = fake(Git::new, github_checkout());
            let (github, _) = fake(GitHub::new, vec![listed(state)]);
            let (herdr, herdr_runner) = fake(Herdr::new, vec![]);

            let error = complete_task(
                &storage,
                &git,
                &github,
                &herdr,
                merged_with(extra.id, MergedWithRef::Task(task.id.0)),
            )
            .unwrap_err();

            assert_eq!(error.kind, CommandErrorKind::Rejected);
            assert!(error.message.contains(reason), "{}", error.message);
            assert!(herdr_runner.invocations.lock().unwrap().is_empty());
            assert_eq!(stored(&storage, extra.id), extra);
        }
    }

    fn conclusion(id: TaskId, note: &str) -> ConcludeTask {
        ConcludeTask {
            id,
            note: note.to_owned(),
        }
    }

    #[test]
    fn conclude_finishes_a_task_without_a_pull_request_as_not_adopted() {
        let (storage, task, dependent) = started(false);
        let (git, _) = fake(Git::new, github_checkout());
        let (github, gh_runner) = fake(GitHub::new, vec![Ok("[]".to_owned())]);
        let (herdr, herdr_runner) = fake(Herdr::new, vec![Ok(CLOSED.to_owned())]);

        let done = conclude_task(
            &storage,
            &git,
            &github,
            &herdr,
            conclusion(task.id, " Duplicate of #7 "),
        )
        .unwrap();

        assert_eq!(done.status, types::TaskStatus::Done);
        assert_eq!(done.resolution, Some(types::TaskResolution::NotAdopted));
        assert_eq!(done.completion_note.as_deref(), Some("Duplicate of #7"));
        assert_eq!(last_status(&done), Some(types::TaskStatus::Done));
        assert_eq!(stored(&storage, task.id), done);
        assert_eq!(done.pull_request_url, None);
        assert_eq!(gh_runner.invocations.lock().unwrap().len(), 1);
        assert_eq!(herdr_runner.invocations.lock().unwrap().len(), 1);
        assert_eq!(
            stored(&storage, dependent.id).status,
            types::TaskStatus::Planned
        );
    }

    #[test]
    fn conclude_is_rejected_without_a_note_and_while_a_pull_request_is_open() {
        let (storage, task, _) = started(true);
        let (no_git, _) = fake(Git::new, vec![]);
        let (no_github, _) = fake(GitHub::new, vec![]);
        let (herdr, herdr_runner) = fake(Herdr::new, vec![]);

        let error = conclude_task(
            &storage,
            &no_git,
            &no_github,
            &herdr,
            conclusion(task.id, " \n"),
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);

        let (git, _) = fake(Git::new, github_checkout());
        let (github, gh_runner) = fake(GitHub::new, vec![listed("OPEN")]);
        let error = conclude_task(
            &storage,
            &git,
            &github,
            &herdr,
            conclusion(task.id, "Not adopted"),
        )
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(error.message.contains("is still open"), "{}", error.message);
        // Only the state was read; the pull request was not closed.
        assert_eq!(gh_runner.invocations.lock().unwrap().len(), 1);
        assert!(herdr_runner.invocations.lock().unwrap().is_empty());
        assert_eq!(stored(&storage, task.id), task);
    }

    #[test]
    fn conclude_is_rejected_for_a_pull_request_toodo_has_not_recorded_yet() {
        // In progress: the background check has not adopted the pull request yet.
        let (storage, task, _) = started(false);
        assert_eq!(task.pull_request_url, None);
        for (state, reason) in [
            ("OPEN", "is still open"),
            ("MERGED", "completes with the merge"),
        ] {
            let (git, _) = fake(Git::new, github_checkout());
            let (github, gh_runner) = fake(GitHub::new, vec![listed(state)]);
            let (herdr, herdr_runner) = fake(Herdr::new, vec![]);

            let error = conclude_task(
                &storage,
                &git,
                &github,
                &herdr,
                conclusion(task.id, "Not adopted"),
            )
            .unwrap_err();

            assert_eq!(error.kind, CommandErrorKind::Rejected);
            assert!(error.message.contains(reason), "{}", error.message);
            assert_eq!(gh_runner.invocations.lock().unwrap().len(), 1);
            assert!(herdr_runner.invocations.lock().unwrap().is_empty());
            assert_eq!(stored(&storage, task.id), task);
        }
    }
}
