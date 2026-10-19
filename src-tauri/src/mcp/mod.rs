//! MCP server for coding agents, embedded in the app so it shares the database lock and
//! tells the frontend about changes like any command.
//!
//! It serves Streamable HTTP on a fixed loopback port and requires the bearer token kept
//! in the app data directory. Agents can read and add workspaces, read and set the
//! worktree root of each machine, read tasks, search earlier tasks
//! and read their history, create and edit tasks, connect dependencies and related
//! links, leave progress and blocker notes in a task's history, and
//! start, complete, or conclude tasks through the app's own commands; deleting tasks stays
//! in the app. The `toodo` skill, the
//! workflow for working on a task, is served by the `read_toodo_skill` tool rather than
//! as a resource, since not every client can read resources. Agents on SSH machines reach
//! the server through the reverse tunnels of [`tunnels`].

mod tools;
pub mod tunnels;

use std::fs;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::commands::events::Notifier;
use crate::commands::types::Task;
use crate::commands::{CommandResult, StorageState, lock};
use crate::domain::model;

pub const PORT: u16 = 47615;

pub(crate) const TOKEN_FILE: &str = "mcp-token";

const SKILL: &str = include_str!("toodo-skill.md");

const INSTRUCTIONS: &str = "Toodo plans tasks as a dependency DAG across workspaces; \
a task becomes ready only when all tasks it depends on are done.

Before you work on a Toodo task or create tasks, read the toodo skill unless you \
already have it: call the read_toodo_skill tool. It is the workflow from the task description to a pull \
request into the workspace base branch, including when to stop for prerequisite tasks, \
and the format of task descriptions.

How to use these tools:
1. Call current_task with your absolute working directory first. Use its workspaceId \
for new tasks in the same repository and its baseBranch as the base of your pull \
request. If it finds no task, use list_workspaces to pick the workspace. Add a \
workspace with create_workspace only when the user asks for a repository that \
list_workspaces does not have yet.
2. Before you plan work or call create_task, call search_tasks with a few distinctive \
words of the work (all workspaces unless the work clearly belongs to one) to find \
earlier tasks about it, and get_task on the relevant ones to read what was decided \
then: description, notes, completion note (why research was not adopted), pull \
request, and commits. Follow their depends_on, dependents, and related links with \
get_task to understand the history. Reuse a matching open task instead of creating a \
duplicate. When a new task builds on earlier context but does not have to wait for it, \
link them with add_related; use add_dependency only for a real prerequisite (rule 3). \
Related links never block starting a task; remove_related drops one.
3. A dependency points from a task to the tasks that must be done before it: task_id is \
the dependent task and depends_on its prerequisites; self and cyclic dependencies are \
rejected. Decide the direction by what has to finish first:
   - Work that builds on your task once it is done (a follow-up feature, the next \
step): create_task with depends_on set to your task's id.
   - Work your task needs before it can be finished (e.g. fixes found while verifying \
it): create those tasks without depending on your task, then call add_dependency with \
task_id = your task and depends_on = all of the new task ids. Never make them depend on \
your task: your task could then never finish.
   Adding a prerequisite that is not done to an in_progress task pauses it. Its herdr \
workspace stays open, and the Toodo app asks the user whether to close it, which ends \
your session; the user resumes the task in the app once its prerequisites are done. So \
create every task first, tell the user what you planned and why, make that \
add_dependency call last, with all prerequisites in one call, and stop working on the \
task after it. Use remove_dependency to drop an edge.
4. Pick task_type by the kind of change (feat, fix, modify, refactor, docs, test, chore, \
rnd), write a short imperative title, and put details in description. Write the \
description in GitHub-flavored Markdown (Toodo renders it and uses it as the GitHub \
issue body) with three sections: `## Purpose` (the outcome and why), `## Background` \
(context, what prompted it, related tasks, constraints), and `## Done when` (a `- [ ]` \
task list of verifiable criteria). Use `-` lists, backticks for code, paths, and \
commands, fenced code blocks, and links; no raw HTML. \
When you split work into tasks, set estimate to your expected size of each (hours, \
days, weeks, or months of active work); the user's reviews compare it \
with the time the task actually took. Set due_date (YYYY-MM-DD) only when the user named \
a deadline.
5. Leave a record of your work on your task as you go; it feeds the user's reviews of \
their work. Call add_progress_note when you take a decision worth remembering (the choice \
and why, e.g. a library or approach over its alternatives) or finish a meaningful step, \
and report_blocker when something stops you that needs the user or someone else (missing \
access, an unclear requirement, a failing dependency), saying what you need. Write one \
or two Markdown sentences per note; skip routine steps such as running tests or small \
edits. Neither tool changes the task's status.
6. A tool error says what was rejected; change the arguments instead of retrying them \
unchanged.
7. start_task, complete_task, and conclude_task act on other tasks, the way the user \
would in the Toodo app: start a ready task (or resume a paused one) for another agent \
session, complete a task whose pull request is merged, or conclude one that ends \
without a merge (research not adopted, a duplicate, work no longer needed) with the \
reason. Never start, complete, or conclude your own task, the one current_task finds: \
it was started for you, and the merge of its pull request completes it. conclude_task \
never closes a pull request, so a task with an open one is rejected; the user concludes \
it in the app.
8. start_task needs a worktree root, the directory that holds task worktrees, on the \
machine of the task's workspace. list_machine_settings shows it for every machine. Set \
a missing one with set_worktree_root to a directory the user named, or when start_task \
reports it missing and the user tells you where worktrees go. Never change or clear a \
root that is set unless the user asks.

Deleting tasks happens in the Toodo app only, not through these tools. A pull request \
from the task branch into baseBranch moves the task to in_review, and its merge \
completes the task. Toodo notices both at its next pull request check, which runs every \
minute, so right after a merge the task can still read in_review for up to about a \
minute; that is expected, not a miss. Read it again with get_task after a minute instead \
of calling complete_task. Only a task still in_review after that needs a look: for \
another task, complete_task says what blocks it; for your own, call report_blocker. \
Whether you merge it yourself is set per workspace by agentMerge, \
which current_task returns: with never (the default), stop once the pull request is \
open and never merge it. With afterChecks, wait until every check on the pull request \
has finished and passed (`gh pr checks <number> --watch`), fixing and pushing when one \
fails, then merge it into baseBranch (`gh pr merge <number> --merge`, or the merge \
method the repository allows). Even then, never merge while a check is failing or \
pending or when the pull request has no checks, never bypass branch protection (no \
--admin), never merge a pull request into any other branch, and never release, tag, or \
merge baseBranch onward (e.g. into main).";

/// What the app needs to serve the tools: the database, the adapters the app's commands
/// run, and a way to announce changes.
pub trait Backend: Notifier + Send + Sync + 'static {
    fn storage(&self) -> &StorageState;
    fn git(&self) -> &Git;
    fn github(&self) -> &GitHub;
    fn herdr(&self) -> &Herdr;
}

/// How agents reach the server, shown in the app; `error` tells why it is not running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub url: String,
    pub token: Option<String>,
    pub error: Option<String>,
}

/// The URL agents on this machine reach the server at when it listens on `port`.
pub fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Prepares the server on its fixed loopback port: the access token and the listener to
/// serve [`router`] on. Failures, e.g. the port being taken, are reported in the status
/// rather than stopping the app.
#[cfg(feature = "desktop")]
pub fn open(data_dir: &Path) -> (ServerStatus, Option<(TcpListener, String)>) {
    let url = url(PORT);
    let token = match load_token(&data_dir.join(TOKEN_FILE)) {
        Ok(token) => token,
        Err(error) => {
            let status = ServerStatus {
                url,
                token: None,
                error: Some(format!(
                    "could not read or create the access token: {error}"
                )),
            };
            return (status, None);
        }
    };
    match bind(SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, PORT))) {
        Ok(listener) => {
            let status = ServerStatus {
                url,
                token: Some(token.clone()),
                error: None,
            };
            (status, Some((listener, token)))
        }
        Err(error) => {
            let status = ServerStatus {
                url,
                token: Some(token),
                error: Some(format!("could not listen on port {PORT}: {error}")),
            };
            (status, None)
        }
    }
}

pub(crate) fn bind(address: SocketAddr) -> io::Result<TcpListener> {
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub(crate) async fn serve(listener: TcpListener, app: axum::Router) -> io::Result<()> {
    axum::serve(tokio::net::TcpListener::from_std(listener)?, app).await
}

/// The token in `path`, created with 32 random bytes on first use. On Unix only the owner
/// may read it: a new file is created that way and an existing one is tightened.
pub(crate) fn load_token(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => {
            make_private(path)?;
            return Ok(token.trim().to_owned());
        }
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(path)?, token.as_bytes())?;
    Ok(token)
}

#[cfg(unix)]
fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// The MCP endpoint at `/mcp`, open only to requests that bear `token`.
pub(crate) fn router(backend: Arc<dyn Backend>, token: String) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(ToodoMcp::new(backend.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    axum::Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(
            Arc::new(token),
            require_token,
        ))
}

pub(crate) async fn require_token(
    State(token): State<Arc<String>>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if presented.is_some_and(|presented| same_secret(presented, &token)) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or wrong Toodo access token",
        )
            .into_response()
    }
}

/// Compares without stopping at the first difference, so timing does not leak the token.
fn same_secret(presented: &str, token: &str) -> bool {
    presented.len() == token.len()
        && presented
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[derive(Clone)]
struct ToodoMcp {
    backend: Arc<dyn Backend>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl ToodoMcp {
    fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            tool_router: Self::tool_router(),
        }
    }

    /// Runs a tool that changes a task and announces the change to the frontend.
    fn change(
        &self,
        apply: impl FnOnce(&mut crate::storage::Storage) -> CommandResult<Task>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = apply(&mut lock(self.backend.storage()));
        self.announce(result)
    }

    /// Runs a tool that drives git, gh, or herdr on a blocking thread, since those take
    /// seconds, and announces the change to the frontend.
    async fn change_blocking(
        &self,
        apply: impl FnOnce(&dyn Backend) -> CommandResult<Task> + Send + 'static,
    ) -> Result<CallToolResult, ErrorData> {
        let result = self.blocking(apply).await?;
        self.announce(result)
    }

    /// Runs `apply` on a blocking thread.
    async fn blocking<T: Send + 'static>(
        &self,
        apply: impl FnOnce(&dyn Backend) -> CommandResult<T> + Send + 'static,
    ) -> Result<CommandResult<T>, ErrorData> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || apply(backend.as_ref()))
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }

    fn announce(&self, result: CommandResult<Task>) -> Result<CallToolResult, ErrorData> {
        if let Ok(task) = &result {
            self.backend
                .tasks_changed(model::WorkspaceId(task.workspace_id.0));
        }
        reply(result)
    }

    #[tool(
        description = "Return the toodo skill (Markdown): the workflow for working on a Toodo task. Read it before you work on a task or create tasks."
    )]
    fn read_toodo_skill(&self) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::success(vec![ContentBlock::text(SKILL)]))
    }

    #[tool(description = "List Toodo workspaces (repositories) with their IDs.")]
    fn list_workspaces(&self) -> Result<CallToolResult, ErrorData> {
        reply(tools::list_workspaces(&lock(self.backend.storage())))
    }

    #[tool(
        description = "Register a repository on the machine Toodo runs on as a new workspace, as Clone or Open in the Toodo app does: clone clone_url into path, or, without clone_url, open the repository already cloned at path; then open it in herdr. Returns the workspace with its id. Call list_workspaces first and reuse a workspace that already has the repository; a path another workspace uses is rejected. Only for repositories the user asked to add to Toodo."
    )]
    async fn create_workspace(
        &self,
        Parameters(request): Parameters<tools::CreateWorkspace>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = self
            .blocking(move |backend| {
                tools::create_workspace(backend.storage(), backend.git(), backend.herdr(), request)
            })
            .await?;
        if result.is_ok() {
            self.backend.workspaces_changed();
        }
        reply(result)
    }

    #[tool(
        description = "List the settings of the machine Toodo runs on (machine kind local) and of every known SSH machine (kind ssh with its label, the SSH host): saved in herdr, hosting a workspace, or with saved settings. worktreeRoot is the directory that holds task worktrees there; start_task is rejected for tasks on a machine whose worktreeRoot is null. herdrError tells why herdr's saved machines could not be read."
    )]
    async fn list_machine_settings(&self) -> Result<CallToolResult, ErrorData> {
        let result = self
            .blocking(|backend| tools::list_machine_settings(backend.storage(), backend.herdr()))
            .await?;
        reply(result)
    }

    #[tool(
        description = "Set or clear the worktree root of one machine, as the worktree root setting in the Toodo app does: the absolute directory in which Toodo creates the worktrees of tasks on that machine, as <root>/<workspace>/<type>-<title>-<id>. Omit ssh_host for the machine Toodo runs on. Set it when start_task reports a missing worktree root or the user asks; use the directory the user named, and never change or clear a root that is already set unless the user asks. Only later starts use the new root. Returns the machine's settings."
    )]
    fn set_worktree_root(
        &self,
        Parameters(request): Parameters<tools::SetWorktreeRoot>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::set_worktree_root(
            &mut lock(self.backend.storage()),
            request,
        ))
    }

    #[tool(
        description = "List tasks with status and depends_on, of one workspace or of all workspaces."
    )]
    fn list_tasks(
        &self,
        Parameters(request): Parameters<tools::ListTasks>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::list_tasks(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Search tasks of all workspaces (or one) by words in their title, description, notes, completion note, branch, and pull request, best match first. Every word must appear, as a substring in any language; words are literal, not operators. Use it before planning or create_task to find earlier tasks on the same subject, then get_task to read what was decided. Each result lists its depends_on, dependents, and related task IDs."
    )]
    fn search_tasks(
        &self,
        Parameters(request): Parameters<tools::SearchTasks>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::search_tasks(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Read one task with its whole history: description, status timeline, progress and blocker notes, completion note (the conclusion of a task done as not_adopted), pull request, commits, and the tasks it depends on, that depend on it, and that are related to it (id, title, status). Use it to learn what an earlier task decided, and follow its links with further get_task calls."
    )]
    fn get_task(
        &self,
        Parameters(request): Parameters<tools::GetTask>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::task_details(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Link two tasks, from any workspaces, as related: context worth reading together, e.g. an earlier task whose decisions a new one builds on. The link goes both ways and never blocks either task; for work that must be done first, use add_dependency instead. Returns task_id's details."
    )]
    fn add_related(
        &self,
        Parameters(request): Parameters<tools::Related>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::add_related(
            &mut lock(self.backend.storage()),
            request,
        ))
    }

    #[tool(
        description = "Remove the related link between two tasks, given in either order. Returns task_id's details."
    )]
    fn remove_related(
        &self,
        Parameters(request): Parameters<tools::Related>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::remove_related(
            &mut lock(self.backend.storage()),
            request,
        ))
    }

    #[tool(description = "Find the Toodo task whose worktree contains the given directory.")]
    fn current_task(
        &self,
        Parameters(request): Parameters<tools::CurrentTask>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::current_task(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Create a planned task in a workspace, optionally depending on existing tasks."
    )]
    fn create_task(
        &self,
        Parameters(request): Parameters<tools::CreateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::create_task(storage, request))
    }

    #[tool(
        description = "Change the type, title, description, estimate, or due date of a task; omitted fields stay unchanged."
    )]
    fn update_task(
        &self,
        Parameters(request): Parameters<tools::UpdateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::update_task(storage, request))
    }

    #[tool(
        description = "Make task_id depend on every task in depends_on. Rejects self and cyclic dependencies; one rejected prerequisite adds none. If task_id is in_progress and a prerequisite is not done, the task is paused; the user then decides in the Toodo app whether to close its herdr workspace, which ends your session."
    )]
    fn add_dependency(
        &self,
        Parameters(request): Parameters<tools::AddDependencies>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::add_dependencies(storage, request))
    }

    #[tool(
        description = "Record progress or a decision (what and why) on a task's history, e.g. your current task. Returns the task with its notes."
    )]
    fn add_progress_note(
        &self,
        Parameters(request): Parameters<tools::AddNote>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::add_note(storage, model::NoteKind::Progress, request))
    }

    #[tool(
        description = "Record on a task's history what blocks the work on it and what is needed to continue. Does not change the task's status. Returns the task with its notes."
    )]
    fn report_blocker(
        &self,
        Parameters(request): Parameters<tools::AddNote>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::add_note(storage, model::NoteKind::Blocker, request))
    }

    #[tool(
        description = "Start a planned task whose prerequisites are all done, as Start (or Resume for a paused task) in the Toodo app does: create or reopen its worktree and herdr workspace and move it to in_progress. Use it to hand a ready task to a new agent session; never start your own task, the one current_task finds. Rejected with the reason when the task is not ready, is not planned, or its machine has no worktree root (set it with set_worktree_root)."
    )]
    async fn start_task(
        &self,
        Parameters(request): Parameters<tools::TaskRef>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_blocking(move |backend| {
            tools::start_task(backend.storage(), backend.git(), backend.herdr(), request)
        })
        .await
    }

    #[tool(
        description = "Complete an in_review task whose pull request is merged, as the Toodo app does: the task becomes done (merged), its herdr workspace is closed, and its dependents can start. Rejected while the pull request is not merged. Toodo also completes merged tasks on its own within a minute. Never complete your own task, the one current_task finds: the merge of its pull request completes it."
    )]
    async fn complete_task(
        &self,
        Parameters(request): Parameters<tools::TaskRef>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_blocking(move |backend| {
            tools::complete_task(
                backend.storage(),
                backend.github(),
                backend.herdr(),
                request,
            )
        })
        .await
    }

    #[tool(
        description = "Finish an in_progress or in_review task without merging, as Conclude in the Toodo app does: the task becomes done (not_adopted) with note, the reason, as its completion note, its herdr workspace is closed, and its dependents stay blocked. For research that is not adopted, a duplicate, or work no longer needed. Rejected when note is empty, when its branch has a merged pull request, and while its branch has an open pull request, even one Toodo has not recorded yet, which only the user closes, by concluding the task in the app. Never conclude your own task, the one current_task finds."
    )]
    async fn conclude_task(
        &self,
        Parameters(request): Parameters<tools::ConcludeTask>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_blocking(move |backend| {
            tools::conclude_task(
                backend.storage(),
                backend.git(),
                backend.github(),
                backend.herdr(),
                request,
            )
        })
        .await
    }

    #[tool(description = "Remove the dependency of task_id on depends_on.")]
    fn remove_dependency(
        &self,
        Parameters(request): Parameters<tools::Dependency>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::remove_dependency(storage, request))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ToodoMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Tool failures are tool-level errors, so the agent sees the reason, e.g. a cycle.
fn reply<T: Serialize>(result: CommandResult<T>) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(value) => {
            let json = serde_json::to_string_pretty(&value)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
        }
        Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
            "{:?}: {}",
            error.kind, error.message
        ))])),
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn the_token_is_created_once_and_then_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TOKEN_FILE);
        let token = load_token(&path).unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(load_token(&path).unwrap(), token);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let created = dir.path().join(TOKEN_FILE);
        load_token(&created).unwrap();
        assert_eq!(mode(&created), 0o600);

        let existing = dir.path().join("existing");
        fs::write(&existing, "abc").unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o664)).unwrap();
        assert_eq!(load_token(&existing).unwrap(), "abc");
        assert_eq!(mode(&existing), 0o600);

        let blank = dir.path().join("blank");
        fs::write(&blank, "\n").unwrap();
        fs::set_permissions(&blank, fs::Permissions::from_mode(0o664)).unwrap();
        assert_eq!(load_token(&blank).unwrap().len(), 64);
        assert_eq!(mode(&blank), 0o600);
    }

    struct TestBackend {
        storage: StorageState,
        git: Git,
        github: GitHub,
        herdr: Herdr,
        changed: std::sync::Mutex<Vec<model::WorkspaceId>>,
        workspaces_changed: std::sync::Mutex<usize>,
    }

    type Scripted = Vec<Result<String, crate::adapters::git::GitError>>;

    impl TestBackend {
        fn new(storage: crate::storage::Storage) -> Self {
            Self::scripted(storage, Vec::new(), Vec::new())
        }

        /// Git and herdr return `git` and `herdr` in order; gh must not run.
        fn scripted(storage: crate::storage::Storage, git: Scripted, herdr: Scripted) -> Self {
            use crate::adapters::git::fake::FakeRunner;
            let runner = |results: Scripted| Box::new(Arc::new(FakeRunner::new(results)));
            Self {
                storage: std::sync::Mutex::new(storage),
                git: Git::new(runner(git)),
                github: GitHub::new(runner(Vec::new())),
                herdr: Herdr::new(runner(herdr)),
                changed: std::sync::Mutex::default(),
                workspaces_changed: std::sync::Mutex::default(),
            }
        }
    }

    impl Notifier for TestBackend {
        fn tasks_changed(&self, workspace: model::WorkspaceId) {
            self.changed.lock().unwrap().push(workspace);
        }
        fn workspaces_changed(&self) {
            *self.workspaces_changed.lock().unwrap() += 1;
        }
        fn agent_statuses_changed(&self, _: &[crate::commands::types::TaskAgentStatus]) {}
        fn pull_request_statuses_changed(
            &self,
            _: &[crate::commands::types::TaskPullRequestStatus],
        ) {
        }
    }

    impl Backend for TestBackend {
        fn storage(&self) -> &StorageState {
            &self.storage
        }

        fn git(&self) -> &Git {
            &self.git
        }

        fn github(&self) -> &GitHub {
            &self.github
        }

        fn herdr(&self) -> &Herdr {
            &self.herdr
        }
    }

    /// A database with one local workspace and one planned task, and the machine's
    /// worktree root if `root` is given.
    fn storage_with_task(root: Option<&str>) -> (crate::storage::Storage, model::Task) {
        use crate::domain::model::{MachineSettings, TaskType, WorkspaceLocation};
        use crate::domain::path::WindowsPath;
        let mut storage = crate::storage::Storage::open_in_memory().unwrap();
        let workspace = storage
            .create_workspace(&crate::storage::NewWorkspace {
                name: "toodo".to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap().into(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        storage
            .save_machine_settings(&MachineSettings::Local {
                worktree_root: root.map(|root| WindowsPath::parse(root).unwrap().into()),
            })
            .unwrap();
        let task = storage
            .create_task(&crate::storage::NewTask {
                workspace_id: workspace.id,
                task_type: TaskType::Feat,
                title: "Add login".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
            })
            .unwrap();
        (storage, task)
    }

    fn start(server: &ToodoMcp, id: model::TaskId) -> serde_json::Value {
        let request = tools::TaskRef { id: id.into() };
        let result = runtime()
            .block_on(server.start_task(Parameters(request)))
            .unwrap();
        serde_json::to_value(result).unwrap()
    }

    #[test]
    fn starting_a_ready_task_moves_it_in_progress_and_announces_it() {
        let (storage, task) = storage_with_task(Some(r"D:\wt"));
        let created =
            r#"{"id":"cli:worktree:create","result":{"workspace":{"workspace_id":"w27"}}}"#;
        let backend = Arc::new(TestBackend::scripted(
            storage,
            vec![
                Ok("abc\n".to_owned()),
                Ok(String::new()),
                Err(crate::adapters::git::fake::failed()),
                Ok(String::new()),
            ],
            vec![Ok(created.to_owned())],
        ));
        let server = ToodoMcp::new(backend.clone());

        let result = start(&server, task.id);

        assert_ne!(result["isError"], true, "{result}");
        let started: serde_json::Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(started["status"], "in_progress");
        assert_eq!(started["herdrWorkspaceId"], "w27");
        let stored = lock(&backend.storage).get_task(task.id).unwrap().unwrap();
        let statuses: Vec<_> = stored.timeline.iter().map(|event| event.status).collect();
        assert_eq!(
            statuses,
            [
                crate::domain::status::TaskStatus::Planned,
                crate::domain::status::TaskStatus::InProgress
            ]
        );
        assert_eq!(*backend.changed.lock().unwrap(), [task.workspace_id]);
    }

    #[test]
    fn a_rejected_start_returns_the_reason_as_tool_error_text_and_announces_nothing() {
        let (storage, task) = storage_with_task(None);
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());

        let result = start(&server, task.id);

        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "InvalidInput: worktree root is not set for machine local; ask the user where task \
             worktrees go on that machine, call set_worktree_root with worktree_root set to \
             that absolute directory, then start the task again"
        );
        assert!(backend.changed.lock().unwrap().is_empty());
        let stored = lock(&backend.storage).get_task(task.id).unwrap().unwrap();
        assert_eq!(stored, task);
    }

    fn create_workspace(server: &ToodoMcp, arguments: serde_json::Value) -> serde_json::Value {
        let request = serde_json::from_value(arguments).unwrap();
        let result = runtime()
            .block_on(server.create_workspace(Parameters(request)))
            .unwrap();
        serde_json::to_value(result).unwrap()
    }

    /// The local path for POSIX `posix` in this build: itself on the server, the same path
    /// on drive C: in the desktop app.
    fn host(posix: &str) -> String {
        if cfg!(feature = "server") {
            posix.to_owned()
        } else {
            format!("C:{}", posix.replace('/', "\\"))
        }
    }

    /// What `git rev-parse --show-toplevel` prints for `posix` in this build.
    fn toplevel(posix: &str) -> Result<String, crate::adapters::git::GitError> {
        Ok(if cfg!(feature = "server") {
            format!("{posix}\n")
        } else {
            format!("C:{posix}\n")
        })
    }

    /// `herdr workspace list` without the repository, then `herdr workspace create`.
    fn herdr_opens_a_new_workspace() -> Scripted {
        vec![
            Ok(r#"{"id":"cli:workspace:list","result":{"workspaces":[]}}"#.to_owned()),
            Ok(
                r#"{"id":"cli:workspace:create","result":{"workspace":{"workspace_id":"w9"}}}"#
                    .to_owned(),
            ),
        ]
    }

    fn created_workspace(result: &serde_json::Value) -> serde_json::Value {
        assert_ne!(result["isError"], true, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    fn error_text(result: &serde_json::Value) -> &str {
        assert_eq!(result["isError"], true, "{result}");
        result["content"][0]["text"].as_str().unwrap()
    }

    /// A server whose database has the workspace "site" at /dev/site, opened through the
    /// tool; git and herdr then return `git` and `herdr`.
    fn server_with_site(git: Scripted, herdr: Scripted) -> (Arc<TestBackend>, ToodoMcp) {
        let (storage, _) = storage_with_task(None);
        let mut git_results = vec![toplevel("/dev/site")];
        git_results.extend(git);
        let mut herdr_results = herdr_opens_a_new_workspace();
        herdr_results.extend(herdr);
        let backend = Arc::new(TestBackend::scripted(storage, git_results, herdr_results));
        let server = ToodoMcp::new(backend.clone());
        let result = create_workspace(
            &server,
            serde_json::json!({
                "name": "site",
                "path": host("/dev/site/src"),
                "default_base_branch": "dev",
            }),
        );
        created_workspace(&result);
        (backend, server)
    }

    fn workspace_count(backend: &TestBackend) -> usize {
        lock(&backend.storage).list_workspaces().unwrap().len()
    }

    #[test]
    fn creating_a_workspace_opens_an_existing_clone_at_its_root() {
        let (storage, _) = storage_with_task(None);
        let backend = Arc::new(TestBackend::scripted(
            storage,
            vec![toplevel("/dev/site")],
            herdr_opens_a_new_workspace(),
        ));
        let server = ToodoMcp::new(backend.clone());

        let result = create_workspace(
            &server,
            serde_json::json!({
                "name": "site",
                "path": host("/dev/site/src"),
                "default_base_branch": "dev",
            }),
        );

        let workspace = created_workspace(&result);
        assert_eq!(workspace["name"], "site");
        assert_eq!(
            workspace["location"],
            serde_json::json!({"kind": "local", "path": host("/dev/site")})
        );
        assert_eq!(workspace["defaultBaseBranch"], "dev");
        let id = model::WorkspaceId(workspace["id"].as_i64().unwrap());
        let stored = lock(&backend.storage).get_workspace(id).unwrap().unwrap();
        assert_eq!(stored.name, "site");
        assert_eq!(*backend.workspaces_changed.lock().unwrap(), 1);
    }

    #[test]
    fn creating_a_workspace_clones_the_url_and_detects_the_base_branch() {
        let (storage, _) = storage_with_task(None);
        let git_runner = Arc::new(crate::adapters::git::fake::FakeRunner::new(vec![
            Ok(String::new()),
            Ok("origin/main\n".to_owned()),
        ]));
        let mut backend = TestBackend::scripted(storage, Vec::new(), herdr_opens_a_new_workspace());
        backend.git = Git::new(Box::new(git_runner.clone()));
        let backend = Arc::new(backend);
        let server = ToodoMcp::new(backend.clone());

        let result = create_workspace(
            &server,
            serde_json::json!({
                "name": "site",
                "path": host("/dev/site"),
                "clone_url": "git@github.com:me/site.git",
            }),
        );

        let workspace = created_workspace(&result);
        assert_eq!(workspace["location"]["path"], host("/dev/site"));
        assert_eq!(workspace["defaultBaseBranch"], "main");
        let invocations = git_runner.invocations.lock().unwrap();
        assert_eq!(invocations[0].args[0], "clone");
        assert!(
            invocations[0]
                .args
                .contains(&"git@github.com:me/site.git".to_owned())
        );
        assert_eq!(*backend.workspaces_changed.lock().unwrap(), 1);
    }

    #[test]
    fn a_path_another_workspace_uses_is_rejected_before_anything_runs() {
        // Neither git nor herdr may run again: their scripts end after the first workspace.
        let (backend, server) = server_with_site(Vec::new(), Vec::new());

        let result = create_workspace(
            &server,
            serde_json::json!({
                "name": "again",
                "path": host("/dev/site"),
                "clone_url": "git@github.com:me/site.git",
            }),
        );

        assert_eq!(
            error_text(&result),
            format!(
                "Rejected: workspace site (2) already uses {}",
                host("/dev/site")
            )
        );
        assert_eq!(workspace_count(&backend), 2);
        assert_eq!(*backend.workspaces_changed.lock().unwrap(), 1);
    }

    #[test]
    fn opening_another_folder_of_a_workspace_repository_is_rejected_before_herdr_runs() {
        let (backend, server) = server_with_site(vec![toplevel("/dev/site")], Vec::new());

        let result = create_workspace(
            &server,
            serde_json::json!({
                "name": "again",
                "path": host("/dev/site/docs"),
                "default_base_branch": "main",
            }),
        );

        assert_eq!(
            error_text(&result),
            format!(
                "Rejected: workspace site (2) already uses {}",
                host("/dev/site")
            )
        );
        assert_eq!(workspace_count(&backend), 2);
    }

    #[test]
    fn an_invalid_path_or_a_folder_outside_a_repository_creates_no_workspace() {
        let (storage, _) = storage_with_task(None);
        let backend = Arc::new(TestBackend::scripted(
            storage,
            vec![Err(crate::adapters::git::GitError::Failed {
                command: format!("git -C {} rev-parse --show-toplevel", host("/dev/notes")),
                stderr: "fatal: not a git repository (or any of the parent directories): .git"
                    .to_owned(),
            })],
            Vec::new(),
        ));
        let server = ToodoMcp::new(backend.clone());

        let relative = create_workspace(
            &server,
            serde_json::json!({"name": "notes", "path": "notes"}),
        );
        assert_eq!(
            error_text(&relative),
            r#"InvalidInput: path is not absolute: "notes""#
        );

        let outside = create_workspace(
            &server,
            serde_json::json!({"name": "notes", "path": host("/dev/notes")}),
        );
        let message = error_text(&outside);
        assert!(message.starts_with("External: "), "{message}");
        assert!(message.contains("not a git repository"), "{message}");

        assert_eq!(workspace_count(&backend), 1);
        assert_eq!(*backend.workspaces_changed.lock().unwrap(), 0);
    }

    /// Adds the workspace "site" on the SSH machine devbox with one planned task.
    fn add_devbox_task(storage: &mut crate::storage::Storage) -> model::Task {
        use crate::domain::model::{MachineLabel, TaskType, WorkspaceLocation};
        use crate::domain::path::PosixPath;
        let workspace = storage
            .create_workspace(&crate::storage::NewWorkspace {
                name: "site".to_owned(),
                location: WorkspaceLocation::Ssh {
                    machine: MachineLabel::parse("devbox").unwrap(),
                    path: PosixPath::parse("/srv/site").unwrap(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        storage
            .create_task(&crate::storage::NewTask {
                workspace_id: workspace.id,
                task_type: TaskType::Fix,
                title: "Fix footer".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
            })
            .unwrap()
    }

    /// `herdr machine list --json` with the given SSH targets.
    fn saved_machines(targets: &[&str]) -> Result<String, crate::adapters::git::GitError> {
        let rows: Vec<_> = targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                serde_json::json!({"id": format!("m{index}"), "label": target, "target": target,
                    "session": "default", "enabled": true, "selected": false})
            })
            .collect();
        Ok(serde_json::Value::Array(rows).to_string())
    }

    fn list_machine_settings(server: &ToodoMcp) -> serde_json::Value {
        let result = runtime().block_on(server.list_machine_settings()).unwrap();
        created_workspace(&serde_json::to_value(result).unwrap())
    }

    fn set_worktree_root(server: &ToodoMcp, arguments: serde_json::Value) -> serde_json::Value {
        let request = serde_json::from_value(arguments).unwrap();
        serde_json::to_value(server.set_worktree_root(Parameters(request)).unwrap()).unwrap()
    }

    fn local(root: Option<&str>) -> serde_json::Value {
        serde_json::json!({"machine": {"kind": "local"}, "worktreeRoot": root.map(host)})
    }

    fn ssh(label: &str, root: Option<&str>) -> serde_json::Value {
        serde_json::json!({"machine": {"kind": "ssh", "label": label}, "worktreeRoot": root})
    }

    #[test]
    fn worktree_roots_of_local_and_ssh_machines_are_listed_set_and_cleared() {
        let (mut storage, _) = storage_with_task(None);
        add_devbox_task(&mut storage);
        let backend = Arc::new(TestBackend::scripted(
            storage,
            Vec::new(),
            vec![
                saved_machines(&["gpu", "devbox"]),
                saved_machines(&["gpu"]),
                Err(crate::adapters::git::fake::failed()),
            ],
        ));
        let server = ToodoMcp::new(backend.clone());

        assert_eq!(
            list_machine_settings(&server),
            serde_json::json!({"machines": [local(None), ssh("devbox", None), ssh("gpu", None)]})
        );

        let saved = set_worktree_root(&server, serde_json::json!({"worktree_root": host("/wt")}));
        assert_eq!(created_workspace(&saved), local(Some("/wt")));
        let saved = set_worktree_root(
            &server,
            serde_json::json!({"ssh_host": "devbox", "worktree_root": "/srv/wt/"}),
        );
        assert_eq!(created_workspace(&saved), ssh("devbox", Some("/srv/wt")));
        // A machine known only by its saved settings is listed too.
        set_worktree_root(
            &server,
            serde_json::json!({"ssh_host": "lab", "worktree_root": "/lab"}),
        );
        assert_eq!(
            list_machine_settings(&server),
            serde_json::json!({"machines": [
                local(Some("/wt")),
                ssh("devbox", Some("/srv/wt")),
                ssh("gpu", None),
                ssh("lab", Some("/lab")),
            ]})
        );

        let cleared = set_worktree_root(&server, serde_json::json!({}));
        assert_eq!(created_workspace(&cleared), local(None));
        let cleared = set_worktree_root(
            &server,
            serde_json::json!({"ssh_host": "devbox", "worktree_root": null}),
        );
        assert_eq!(created_workspace(&cleared), ssh("devbox", None));
        // Without herdr's saved machines, the others are still listed.
        let listed = list_machine_settings(&server);
        assert!(listed["herdrError"].is_string(), "{listed}");
        assert_eq!(
            listed["machines"],
            serde_json::json!([local(None), ssh("devbox", None), ssh("lab", Some("/lab"))])
        );
    }

    #[test]
    fn invalid_worktree_roots_and_hosts_are_rejected_and_save_nothing() {
        let (mut storage, _) = storage_with_task(None);
        add_devbox_task(&mut storage);
        let backend = Arc::new(TestBackend::scripted(
            storage,
            Vec::new(),
            vec![saved_machines(&[])],
        ));
        let server = ToodoMcp::new(backend.clone());

        for arguments in [
            serde_json::json!({"worktree_root": "worktrees"}),
            serde_json::json!({"worktree_root": ""}),
            serde_json::json!({"ssh_host": "devbox", "worktree_root": r"D:\wt"}),
            serde_json::json!({"ssh_host": "devbox", "worktree_root": "srv/wt"}),
            serde_json::json!({"ssh_host": " ", "worktree_root": "/wt"}),
        ] {
            let result = set_worktree_root(&server, arguments.clone());
            let message = error_text(&result);
            assert!(
                message.starts_with("InvalidInput: "),
                "{arguments}: {message}"
            );
        }

        assert_eq!(
            list_machine_settings(&server),
            serde_json::json!({"machines": [local(None), ssh("devbox", None)]})
        );
        assert!(
            lock(&backend.storage)
                .ssh_machines_with_settings()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_start_without_the_worktree_root_of_an_ssh_machine_names_the_host_to_set() {
        let (mut storage, _) = storage_with_task(Some(r"D:\wt"));
        let task = add_devbox_task(&mut storage);
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());

        let result = start(&server, task.id);

        assert_eq!(
            error_text(&result),
            "InvalidInput: worktree root is not set for machine devbox; ask the user where \
             task worktrees go on that machine, call set_worktree_root with ssh_host \
             \"devbox\" and worktree_root set to that absolute directory, then start the task \
             again"
        );
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn post_initialize(address: SocketAddr, authorization: &str) -> String {
        use std::io::{Read, Write};
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n{authorization}\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn only_requests_with_the_token_reach_the_tools() {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let address = listener.local_addr().unwrap();
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let app = router(backend, "secret".to_owned());
        std::thread::spawn(move || runtime().block_on(serve(listener, app)));

        assert!(post_initialize(address, "").starts_with("HTTP/1.1 401"));
        let wrong = post_initialize(address, "Authorization: Bearer secreT\r\n");
        assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
        let accepted = post_initialize(address, "Authorization: Bearer secret\r\n");
        assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
        assert!(accepted.contains(r#""name":"toodo""#), "{accepted}");
    }

    #[test]
    fn the_skill_tool_returns_the_skill_text() {
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let server = ToodoMcp::new(Arc::new(TestBackend::new(storage)));
        assert!(server.tool_router.has_route("read_toodo_skill"));
        for tool in [
            "start_task",
            "complete_task",
            "conclude_task",
            "search_tasks",
            "get_task",
            "add_related",
            "remove_related",
        ] {
            assert!(server.tool_router.has_route(tool), "{tool}");
        }
        assert!(!server.tool_router.has_route("delete_task"));
        let result = serde_json::to_value(server.read_toodo_skill().unwrap()).unwrap();
        assert_eq!(result["content"][0]["text"], SKILL);
        assert!(SKILL.starts_with("---") && SKILL.contains("name: toodo"));
        assert_ne!(result["isError"], true);
    }

    #[test]
    fn secrets_match_only_when_equal() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abd", "abc"));
        assert!(!same_secret("ab", "abc"));
    }
}
