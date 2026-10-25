//! MCP server for coding agents, embedded in the app so it shares the database lock and
//! tells the frontend about changes like any command.
//!
//! It serves Streamable HTTP on a fixed loopback port and requires the bearer token kept
//! in the app data directory. Agents can read and add workspaces, read and set the
//! worktree root of each machine, read tasks, search earlier tasks
//! and read their history, create and edit tasks, connect dependencies and related
//! links, leave progress and blocker notes in a task's history, read and keep project
//! memory (decisions, rules, notes, and summaries of a workspace or shared by all), and
//! start, complete, or conclude tasks through the app's own commands; deleting tasks stays
//! in the app. The `toodo` skill, the
//! workflow for working on a task, is served by the `read_toodo_skill` tool rather than
//! as a resource, since not every client can read resources; to the lead the same tool
//! serves the `toodo-lead` skill, its manual for running the work. Agents on SSH machines reach
//! the server through the reverse tunnels of [`tunnels`]. `toodo-server` also accepts
//! its lead's own token, so that what the lead does is recorded and its destructive
//! actions wait for a person's confirmation (see [`lead`]).

mod lead;
mod memory;
mod tools;
pub mod tunnels;

use std::fs;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::Extension;
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

pub(crate) const LEAD_SKILL: &str = include_str!("toodo-lead-skill.md");

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
issue body) with two sections: `## Purpose` (the outcome and why) and `## Background` \
(context, what prompted it, related tasks, constraints). Put what has to hold for the \
task to be done in criteria, not in the description: one verifiable item each, kind \
manual for what is checked by hand, url or command (with target) or pr_open_ci_green \
for what Toodo checks itself. The merge criterion comes first unless you set \
merge_criterion false for work that ships no code. Use `-` lists, backticks for code, \
paths, and commands, fenced code blocks, and links; no raw HTML. \
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
session, complete a task whose completion criteria are met (which confirms one that \
awaits completion), or conclude one that ends without a merge (research not adopted, a \
duplicate, work no longer needed) with the reason. Never start, complete, or conclude \
your own task, the one current_task finds: it was started for you, and meeting its \
criteria completes it. conclude_task \
never closes a pull request, so a task with an open one is rejected; the user concludes \
it in the app. When one pull request ships the work of several tasks, complete each \
extra task with complete_task and merged_with set to the task that owns the pull \
request (or the pull request URL) once it is merged, never with conclude_task: \
not_adopted means the work did not ship. A task already concluded that way can be \
completed with merged_with too.
8. Project memory keeps what a restarted agent needs to continue from Toodo alone: \
decisions (the choice and why), rules to follow, notes, and summaries, for one \
workspace or, without workspace_id, shared by every workspace (the user's standing \
preferences and cross-project rules). Before you plan or act in a workspace, call \
list_memory with its workspace_id and include_shared=true, and follow its rules; use \
search_memory for a subject. When you learn something durable (a decision taken, a \
convention the user stated, how the project is laid out), add it with add_memory, \
linked to your task with task_id; one point per entry, a one-line title, the why in \
the body. When an entry no longer holds, call supersede_memory with the new text \
instead of adding a contradicting entry; it keeps the old one as history. \
update_memory only fixes the wording of an entry that still holds. Task notes \
(add_progress_note) record the history of one task; memory is what stays true \
afterwards.
9. start_task needs a worktree root, the directory that holds task worktrees, on the \
machine of the task's workspace. list_machine_settings shows it for every machine. Set \
a missing one with set_worktree_root to a directory the user named, or when start_task \
reports it missing and the user tells you where worktrees go. Never change or clear a \
root that is set unless the user asks.
10. A task is done when its completion criteria are met. Toodo checks every kind but \
manual itself, about once a minute: pr_merged and pr_open_ci_green from the task's pull \
request, branch_merged_locally from the repository (for workspaces without an origin \
remote, where the task branch is merged into baseBranch locally and nothing is pushed), \
url by fetching its target, and command by running its target in the task worktree once \
every other criterion is met. Check a manual criterion of your task with \
check_criterion once it holds, with evidence (a link, a command and its output, or what \
you verified). Once every criterion is met, a task whose criteria are all automatic is \
done at once; one with a manual criterion awaits completion (in_review) until the user \
confirms it, so stop there. Change the criteria with update_task when the plan \
changes, never to make a task pass.

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

    /// The lead made a request with its token; a server whose lead is outside it tells
    /// from these whether the lead is connected.
    fn lead_contact(&self) {}

    /// A lead outside the server reports that it is composing a reply in the chat, or
    /// that it stopped; returns its status. Only a server with an outside lead has one.
    fn set_lead_typing(&self, _typing: bool) -> CommandResult<crate::commands::lead::LeadStatus> {
        Err(crate::commands::CommandError::rejected(
            "only a lead outside toodo-server (TOODO_LEAD=external) reports that it is typing",
        ))
    }
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
    let token = random_token()?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(path)?, token.as_bytes())?;
    Ok(token)
}

/// 32 random bytes, in hex.
pub(crate) fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(unix)]
pub(crate) fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub(crate) fn make_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// The tokens `/mcp` accepts, and the backend that hears of the lead's requests.
struct Tokens {
    agent: String,
    lead: Option<String>,
    backend: Arc<dyn Backend>,
}

/// The MCP endpoint at `/mcp`, open only to requests that bear `token`, or `lead_token`,
/// which tells the tools that the lead calls them (see [`lead`]).
pub(crate) fn router(
    backend: Arc<dyn Backend>,
    token: String,
    lead_token: Option<String>,
) -> axum::Router {
    let tokens = Arc::new(Tokens {
        agent: token,
        lead: lead_token,
        backend: backend.clone(),
    });
    let service = StreamableHttpService::new(
        move || Ok(ToodoMcp::new(backend.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    axum::Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(tokens, require_caller))
}

/// Lets requests with one of `tokens` through, marked with their [`lead::Caller`]. A
/// request of the lead counts as its contact once it is answered, when the tool it called
/// has changed what it changes.
async fn require_caller(
    State(tokens): State<Arc<Tokens>>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = bearer(request.headers()).unwrap_or("");
    let caller = if same_secret(presented, &tokens.agent) {
        lead::Caller::Agent
    } else if tokens
        .lead
        .as_deref()
        .is_some_and(|lead| same_secret(presented, lead))
    {
        lead::Caller::Lead
    } else {
        return unauthorized();
    };
    request.extensions_mut().insert(caller);
    let response = next.run(request).await;
    if caller == lead::Caller::Lead {
        let backend = tokens.backend.clone();
        // It may read the database, which a tool of another request may hold.
        drop(tokio::task::spawn_blocking(move || backend.lead_contact()).await);
    }
    response
}

/// The bearer token of a request's `Authorization` header.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub(crate) fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        "missing or wrong Toodo access token",
    )
        .into_response()
}

/// Compares without stopping at the first difference, so timing does not leak the token.
pub(crate) fn same_secret(presented: &str, token: &str) -> bool {
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

    /// Runs `apply` on a blocking thread, since git, gh, and herdr take seconds.
    async fn blocking<T: Send + 'static>(
        &self,
        apply: impl FnOnce(&dyn Backend) -> CommandResult<T> + Send + 'static,
    ) -> Result<CommandResult<T>, ErrorData> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || apply(backend.as_ref()))
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }

    /// Tells the frontend that the links of a task changed.
    fn announce_links(&self, result: &CommandResult<tools::TaskDetails>) {
        if let Ok(details) = result {
            self.backend
                .tasks_changed(model::WorkspaceId(details.task.workspace_id.0));
        }
    }

    /// What the tool returns: `result` with, when the lead made the change, `activity` kept
    /// in the task's history and posted to the chat. Recording it can only fail after the
    /// change is done, so a failure is logged rather than returned.
    fn by(
        &self,
        caller: lead::Caller,
        result: CommandResult<Task>,
        activity: impl FnOnce(&Task) -> lead::Activity,
    ) -> CommandResult<Task> {
        let task = result?;
        if caller != lead::Caller::Lead {
            return Ok(task);
        }
        let activity = activity(&task);
        match lead::record(&mut lock(self.backend.storage()), task.id.into(), &activity) {
            Ok(noted) => {
                if activity.chat.is_some() {
                    self.backend.chat_changed();
                }
                Ok(noted)
            }
            Err(error) => {
                eprintln!(
                    "toodo: failed to record what the lead did to task {}: {}",
                    task.id.0, error.message
                );
                Ok(task)
            }
        }
    }

    /// The code a person confirmed a destructive `action` of the lead with in the chat,
    /// or else the tool result telling the lead how to get the confirmation.
    fn confirmed(
        &self,
        tool: &str,
        action: &str,
        confirmation: Option<&str>,
    ) -> Result<String, Result<CallToolResult, ErrorData>> {
        let guarded = lead::guard(
            &mut lock(self.backend.storage()),
            tool,
            action,
            confirmation,
        );
        let error = match guarded {
            Ok(lead::Guard::Confirmed(code)) => return Ok(code),
            Ok(lead::Guard::Refused { error, asked }) => {
                if asked {
                    self.backend.chat_changed();
                }
                error
            }
            Err(error) => error,
        };
        Err(reply::<Task>(Err(error)))
    }

    fn announce(&self, result: CommandResult<Task>) -> Result<CallToolResult, ErrorData> {
        if let Ok(task) = &result {
            self.backend
                .tasks_changed(model::WorkspaceId(task.workspace_id.0));
        }
        reply(result)
    }

    #[tool(
        description = "Return the toodo skill (Markdown): the workflow for working on a Toodo task. Read it before you work on a task or create tasks. The Toodo lead gets its own skill instead: how to turn chat requests into tasks, start coding agents, follow them up, and report."
    )]
    fn read_toodo_skill(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        let skill = match lead::Caller::of(&parts) {
            lead::Caller::Lead => LEAD_SKILL,
            lead::Caller::Agent => SKILL,
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(skill)]))
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
        let result = tools::add_related(&mut lock(self.backend.storage()), request);
        self.announce_links(&result);
        reply(result)
    }

    #[tool(
        description = "Remove the related link between two tasks, given in either order. Returns task_id's details."
    )]
    fn remove_related(
        &self,
        Parameters(request): Parameters<tools::Related>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = tools::remove_related(&mut lock(self.backend.storage()), request);
        self.announce_links(&result);
        reply(result)
    }

    #[tool(
        description = "Post a message to the people in the Toodo chat panel. Only the Toodo lead uses it: it takes their messages with take_pending, and they read only the panel, never the pane. Answer each of them here, and post updates they should see. GitHub-flavored Markdown. Pass handles with the IDs of the messages the reply answers (e.g. [\"chat-12\"]) to mark them handled by it."
    )]
    fn reply_in_chat(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::ChatReply>,
    ) -> Result<CallToolResult, ErrorData> {
        let handles = request.handles.unwrap_or_default();
        if !handles.is_empty() && lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<crate::commands::lead::ChatMessage>(Err(
                crate::commands::CommandError::rejected(
                    "only the Toodo lead marks people's messages handled",
                ),
            ));
        }
        let result = crate::commands::lead::reply(
            &mut lock(self.backend.storage()),
            &request.text,
            &handles,
        );
        if result.is_ok() {
            self.backend.chat_changed();
        }
        reply(result)
    }

    #[tool(
        description = "Only the Toodo lead: acknowledge people's chat messages by ID (e.g. {\"ids\": [\"chat-12\"]}) as soon as you take them: you received them and are on them. The chat panel shows the acknowledgement; it is never an approval. Acknowledging again changes nothing."
    )]
    fn ack_chat(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::AckChat>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<Vec<crate::commands::lead::ChatMessage>>(Err(
                crate::commands::CommandError::rejected(
                    "only the Toodo lead acknowledges people's messages",
                ),
            ));
        }
        let result = crate::commands::lead::ack(
            &mut lock(self.backend.storage()),
            &request.ids.unwrap_or_default(),
        );
        if result.is_ok() {
            self.backend.chat_changed();
        }
        reply(result)
    }

    #[tool(
        description = "Only the Toodo lead: mark a person's chat message handled, with what handled it: reply_id, your reply_in_chat message that answers it, or task_id, the task you created or started for it with a request_id of the message (e.g. chat-12/create-1). Other links are rejected. Usually reply_in_chat with handles does this in one call. A message handled before keeps its first link."
    )]
    fn mark_chat_handled(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::MarkChatHandled>,
    ) -> Result<CallToolResult, ErrorData> {
        use crate::commands::CommandError;
        use crate::domain::chat::{ChatMessageId, HandledBy};
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<crate::commands::lead::ChatMessage>(Err(CommandError::rejected(
                "only the Toodo lead marks people's messages handled",
            )));
        }
        let by = match (request.reply_id, request.task_id) {
            (Some(reply), None) => HandledBy::Reply(ChatMessageId(reply)),
            (None, Some(task)) => HandledBy::Task(task.into()),
            _ => {
                return reply::<crate::commands::lead::ChatMessage>(Err(
                    CommandError::invalid_input("pass exactly one of reply_id and task_id"),
                ));
            }
        };
        let result =
            crate::commands::lead::handle(&mut lock(self.backend.storage()), &request.id, by);
        if result.is_ok() {
            self.backend.chat_changed();
        }
        reply(result)
    }

    #[tool(
        description = "Only the Toodo lead: take the items waiting in its inbox, people's chat messages and the status changes of tasks it follows (in_review, done), oldest first, and mark them taken. Toodo keeps them until taken, while the lead restarts too, and types only their IDs into the lead's pane, as a line starting with [Toodo inbox]. Call it with those ids, and without ids at the start of every session to get what is still waiting. An item with takenBefore was taken by an earlier session, which may have acted on it: name what you do for an item with request_id (create_task, start_task) so doing it again changes nothing."
    )]
    fn take_pending(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::TakePending>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<tools::Pending>(Err(crate::commands::CommandError::rejected(
                "only the Toodo lead takes the items of its inbox",
            )));
        }
        let result = tools::take_pending(&mut lock(self.backend.storage()), request);
        if result
            .as_ref()
            .is_ok_and(|pending| pending.items.iter().any(|item| item.taken_before.is_none()))
        {
            self.backend.chat_changed();
        }
        reply(result)
    }

    #[tool(
        description = "Only the Toodo lead: read the items take_pending without ids would take next (people's chat messages and followed task events, oldest first), without marking them taken. A lead outside Toodo keeps them safe first, then takes them with take_pending and their ids, so that it loses none if it stops in between. open lists people's messages taken before and not handled yet."
    )]
    fn peek_pending(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<tools::Pending>(Err(crate::commands::CommandError::rejected(
                "only the Toodo lead reads the items of its inbox",
            )));
        }
        reply(tools::peek_pending(&lock(self.backend.storage())))
    }

    #[tool(
        description = "Only the Toodo lead: read the chat after a cursor: every message (author user, lead, or system, the system's being Toodo's reports of finished tasks) with an id above after, oldest first, at most limit (100 when omitted, at most 500). Pass the id of the last message read as after to read on; more is true while messages remain. Messages imported from another chat carry origin and originId. It never marks anything taken or read."
    )]
    fn list_chat(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::ListChat>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<tools::ChatFeed>(Err(crate::commands::CommandError::rejected(
                "only the Toodo lead reads the chat feed",
            )));
        }
        reply(tools::list_chat(&lock(self.backend.storage()), &request))
    }

    #[tool(
        description = "Only the Toodo lead: store a message written in another chat that the Toodo chat mirrors: author user (a person's) or lead (yours), its text, origin (a label of the other chat such as matrix: lowercase letters, digits, '.', '_', '-') and origin_id (the message's ID there). The lead already has it, so a person's message never enters the inbox and is never overdue, but it counts like any person's message, e.g. as the answer to a confirmation request. Importing the same origin and origin_id again stores nothing and returns the stored message with created false."
    )]
    fn import_chat_message(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::ImportChatMessage>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<tools::ImportedChat>(Err(crate::commands::CommandError::rejected(
                "only the Toodo lead imports chat messages",
            )));
        }
        let result = tools::import_chat_message(&mut lock(self.backend.storage()), request);
        if result.as_ref().is_ok_and(|imported| imported.created) {
            self.backend.chat_changed();
        }
        reply(result)
    }

    #[tool(
        description = "Only a Toodo lead outside toodo-server (TOODO_LEAD=external): show in the chat that you are composing a reply (typing true), or that you stopped (false). It lasts 30 seconds unless you call it again, and ends once you reply. Returns your status as the chat shows it."
    )]
    fn set_typing(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::SetTyping>,
    ) -> Result<CallToolResult, ErrorData> {
        if lead::Caller::of(&parts) != lead::Caller::Lead {
            return reply::<crate::commands::lead::LeadStatus>(Err(
                crate::commands::CommandError::rejected("only the Toodo lead reports typing"),
            ));
        }
        reply(self.backend.set_lead_typing(request.typing))
    }

    /// Runs a tool that changes project memory and announces the change to the frontend.
    fn change_memory<T: Serialize>(
        &self,
        apply: impl FnOnce(&mut crate::storage::Storage) -> CommandResult<T>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = apply(&mut lock(self.backend.storage()));
        if result.is_ok() {
            self.backend.memory_changed();
        }
        reply(result)
    }

    #[tool(
        description = "List current project memory entries (decision, rule, note, summary), most recently changed first. With workspace_id: that workspace's entries, plus the shared ones when include_shared is true; without it: the entries shared by every workspace. Read it before you plan or act in a workspace. include_superseded adds replaced entries (history)."
    )]
    fn list_memory(
        &self,
        Parameters(request): Parameters<memory::ListMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(memory::list_memory(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Search project memory entries by words in their title and body; every word must appear, as a substring. Same scope as list_memory: workspace_id (plus shared ones with include_shared) or, without it, the shared entries."
    )]
    fn search_memory(
        &self,
        Parameters(request): Parameters<memory::SearchMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(memory::search_memory(
            &lock(self.backend.storage()),
            request,
        ))
    }

    #[tool(
        description = "Read one memory entry with its workspace name and the older entries it superseded, newest first. supersededBy set means the entry no longer holds; read that entry instead."
    )]
    fn get_memory(
        &self,
        Parameters(request): Parameters<memory::GetMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(memory::get_memory(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Add a project memory entry: something that stays true beyond one task. kind: decision (a choice and why), rule (to follow), note, or summary. Give workspace_id for knowledge of that workspace, omit it for knowledge shared by every workspace. Search first and supersede an existing entry instead of adding a contradicting one."
    )]
    fn add_memory(
        &self,
        Parameters(request): Parameters<memory::AddMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_memory(|storage| memory::add_memory(storage, request))
    }

    #[tool(
        description = "Fix the wording of a current memory entry in place; omitted fields stay. For a change of meaning use supersede_memory, which keeps the old text. Superseded entries cannot change."
    )]
    fn update_memory(
        &self,
        Parameters(request): Parameters<memory::UpdateMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_memory(|storage| memory::update_memory(storage, request))
    }

    #[tool(
        description = "Replace a current memory entry that no longer holds with a new one in the same scope; the old entry stays as history, pointing at the new one, and leaves list_memory. Returns the new entry."
    )]
    fn supersede_memory(
        &self,
        Parameters(request): Parameters<memory::SupersedeMemory>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change_memory(|storage| memory::supersede_memory(storage, request))
    }

    #[tool(description = "Find the Toodo task whose worktree contains the given directory.")]
    fn current_task(
        &self,
        Parameters(request): Parameters<tools::CurrentTask>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::current_task(&lock(self.backend.storage()), request))
    }

    #[tool(
        description = "Create a planned task in a workspace with its completion criteria, optionally depending on existing tasks (depends_on) and related to others (related). Without merge_criterion false, its first criterion is the merge: the pull request is merged, or, in a repository without an origin remote, the branch is merged into the base branch locally."
    )]
    async fn create_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::CreateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        let repeated = tools::repeated(
            &lock(self.backend.storage()),
            request.request_id.as_deref(),
            crate::domain::status::TaskStatus::Planned,
        );
        match repeated {
            Ok(Some(task)) => return reply(Ok(task)),
            Ok(None) => {}
            Err(error) => return reply::<Task>(Err(error)),
        }
        let related = request.related.clone();
        let result = self
            .blocking(move |backend| tools::create_task(backend.storage(), backend.git(), request))
            .await?;
        let result = self.by(lead::Caller::of(&parts), result, |task| {
            lead::created(task, &related)
        });
        self.announce(result)
    }

    #[tool(
        description = "Check (met true, the default) or uncheck a manual completion criterion of a task that is not done, with evidence of what shows it holds. Toodo checks the other kinds itself. Once every criterion of an in_progress task is met, it awaits completion (in_review) until the user, or complete_task, confirms it. Returns the task with its criteria."
    )]
    fn check_criterion(
        &self,
        Parameters(request): Parameters<tools::CheckCriterion>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::check_criterion(storage, request))
    }

    #[tool(
        description = "Change the type, title, description, estimate, due date, or completion criteria of a task; omitted fields stay unchanged."
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
        description = "Start a planned task whose prerequisites are all done, as Start (or Resume for a paused task) in the Toodo app does: create or reopen its worktree and herdr workspace and move it to in_progress. Use it to hand a ready task to a new agent session; never start your own task, the one current_task finds. With agent_command, Toodo then runs that command (e.g. the coding agent) in the first pane of the task's herdr workspace. Returns the task with the panes of its herdr workspace, ranIn (the pane agent_command ran in), and paneError when the panes could not be listed or the command did not run. Rejected with the reason when the task is not ready, is not planned, or its machine has no worktree root (set it with set_worktree_root)."
    )]
    async fn start_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::StartTask>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = lead::Caller::of(&parts);
        let repeated = tools::repeated(
            &lock(self.backend.storage()),
            request.request_id.as_deref(),
            crate::domain::status::TaskStatus::InProgress,
        );
        match repeated {
            Ok(Some(task)) => {
                return reply(Ok(tools::TaskStart {
                    task,
                    panes: Vec::new(),
                    ran_in: None,
                    pane_error: None,
                }));
            }
            Ok(None) => {}
            Err(error) => return reply::<tools::TaskStart>(Err(error)),
        }
        let command = request.agent_command.clone();
        let result = self
            .blocking(move |backend| {
                tools::start_task(backend.storage(), backend.git(), backend.herdr(), request)
            })
            .await?;
        let result = result.and_then(|started| {
            let ran = started
                .ran_in
                .clone()
                .zip(command.as_deref().map(str::trim).map(str::to_owned));
            let task = self.by(caller, Ok(started.task), |task| {
                lead::started(
                    task,
                    ran.as_ref()
                        .map(|(pane, command)| (pane.as_str(), command.as_str())),
                )
            })?;
            Ok(tools::TaskStart { task, ..started })
        });
        if let Ok(started) = &result {
            self.backend
                .tasks_changed(model::WorkspaceId(started.task.workspace_id.0));
        }
        reply(result)
    }

    #[tool(
        description = "Complete a task being worked on once its completion criteria are met, as the Toodo app does; this is also how a task awaiting completion because of a manual criterion is confirmed. Toodo re-checks the automatic criteria first; the task becomes done (merged), its herdr workspace is closed, and its dependents can start. Rejected with the criteria not met yet, e.g. while its pull request is not merged. Toodo also completes tasks whose criteria are all automatic on its own within a minute. With merged_with, complete a task whose work shipped with the merged pull request of another task instead, e.g. the extra tasks one pull request fixed: an in_progress or in_review task, or one already done as not_adopted, becomes done (merged) through that pull request, which the app shows on the task. Rejected while that pull request or task is not merged yet, and while the task's own branch has an open or merged pull request. Never complete your own task, the one current_task finds: meeting its criteria completes it."
    )]
    async fn complete_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::CompleteTask>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = lead::Caller::of(&parts);
        let mut confirmed = None;
        if caller == lead::Caller::Lead {
            let task = match tools::get_task(&lock(self.backend.storage()), request.id) {
                Ok(task) => task,
                Err(error) => return reply::<Task>(Err(error)),
            };
            let named = format!("#{} {}", task.id.0, task.title);
            let action = match (&request.merged_with, task.status) {
                (Some(target), _) => Some(format!(
                    "complete {named} as shipped with {}, without a pull request of its own.",
                    target.describe()
                )),
                (None, crate::commands::types::TaskStatus::InReview) => Some(format!(
                    "confirm the completion of {named}, which awaits a person's confirmation \
                     of its criteria."
                )),
                (None, _) => None,
            };
            if let Some(action) = action {
                match self.confirmed("complete_task", &action, request.confirmation.as_deref()) {
                    Ok(code) => confirmed = Some(code),
                    Err(refused) => return refused,
                }
            }
        }
        let result = self
            .blocking(move |backend| {
                tools::complete_task(
                    backend.storage(),
                    backend.git(),
                    backend.github(),
                    backend.herdr(),
                    request,
                )
            })
            .await?;
        let result = self.by(caller, result, |task| {
            lead::completed(task, confirmed.as_deref())
        });
        // The finish posted its report to the chat.
        if result.is_ok() {
            self.backend.chat_changed();
        }
        self.announce(result)
    }

    #[tool(
        description = "Finish an in_progress or in_review task without merging, as Conclude in the Toodo app does: the task becomes done (not_adopted) with note, the reason, as its completion note, its herdr workspace is closed, and its dependents stay blocked. For research that is not adopted, a duplicate, or work no longer needed. Rejected when note is empty, when its branch has a merged pull request, and while its branch has an open pull request, even one Toodo has not recorded yet, which only the user closes, by concluding the task in the app. Never conclude your own task, the one current_task finds."
    )]
    async fn conclude_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(request): Parameters<tools::ConcludeTask>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = lead::Caller::of(&parts);
        let mut confirmed = String::new();
        if caller == lead::Caller::Lead {
            let task = match tools::get_task(&lock(self.backend.storage()), request.id) {
                Ok(task) => task,
                Err(error) => return reply::<Task>(Err(error)),
            };
            let note: Vec<String> = request
                .note
                .trim()
                .lines()
                .map(|line| format!("> {line}").trim_end().to_owned())
                .collect();
            let action = format!(
                "conclude #{} {} without a merge, with this completion note:\n\n{}",
                task.id.0,
                task.title,
                note.join("\n")
            );
            match self.confirmed("conclude_task", &action, request.confirmation.as_deref()) {
                Ok(code) => confirmed = code,
                Err(refused) => return refused,
            }
        }
        let result = self
            .blocking(move |backend| {
                tools::conclude_task(
                    backend.storage(),
                    backend.git(),
                    backend.github(),
                    backend.herdr(),
                    request,
                )
            })
            .await?;
        let result = self.by(caller, result, |_| lead::concluded(&confirmed));
        if result.is_ok() {
            self.backend.chat_changed();
        }
        self.announce(result)
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
        memory_changed: std::sync::Mutex<usize>,
        chat_changed: std::sync::Mutex<usize>,
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
                memory_changed: std::sync::Mutex::default(),
                chat_changed: std::sync::Mutex::default(),
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
        fn memory_changed(&self) {
            *self.memory_changed.lock().unwrap() += 1;
        }
        fn chat_changed(&self) {
            *self.chat_changed.lock().unwrap() += 1;
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
                criteria: Vec::new(),
            })
            .unwrap();
        (storage, task)
    }

    /// The HTTP request of a tool call by `caller`, as the router marks it.
    fn by(caller: lead::Caller) -> Extension<Parts> {
        let mut parts = axum::http::Request::new(()).into_parts().0;
        parts.extensions.insert(caller);
        Extension(parts)
    }

    fn start(server: &ToodoMcp, id: model::TaskId) -> serde_json::Value {
        let request = tools::StartTask {
            id: id.into(),
            agent_command: None,
            request_id: None,
        };
        let result = runtime()
            .block_on(server.start_task(by(lead::Caller::Agent), Parameters(request)))
            .unwrap();
        serde_json::to_value(result).unwrap()
    }

    fn chat(backend: &TestBackend) -> Vec<String> {
        lock(backend.storage())
            .list_chat_messages()
            .unwrap()
            .into_iter()
            .map(|message| message.body)
            .collect()
    }

    fn last_note(task: &serde_json::Value) -> &str {
        task["notes"].as_array().unwrap().last().unwrap()["body"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn what_the_lead_creates_is_linked_noted_and_posted_to_the_chat() {
        let (storage, first) = storage_with_task(None);
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let create = |caller, arguments: serde_json::Value| {
            let request = serde_json::from_value(arguments).unwrap();
            let result = runtime()
                .block_on(server.create_task(by(caller), Parameters(request)))
                .unwrap();
            serde_json::to_value(result).unwrap()
        };
        let task = |title: &str, related: serde_json::Value| {
            serde_json::json!({
                "workspace_id": first.workspace_id.0,
                "task_type": "feat",
                "title": title,
                "depends_on": [first.id.0],
                "related": related,
                "merge_criterion": false,
            })
        };

        let by_agent = created_workspace(&create(
            lead::Caller::Agent,
            task("Add logout", serde_json::json!([])),
        ));
        assert_eq!(by_agent["notes"], serde_json::json!([]));
        assert!(chat(&backend).is_empty());

        let unknown = create(lead::Caller::Lead, task("Lost", serde_json::json!([999])));
        assert!(error_text(&unknown).starts_with("NotFound:"));
        let by_lead = created_workspace(&create(
            lead::Caller::Lead,
            task("Add sessions", serde_json::json!([by_agent["id"]])),
        ));

        let id = by_lead["id"].as_i64().unwrap();
        let agent_id = by_agent["id"].as_i64().unwrap();
        assert_eq!(id, agent_id + 1, "the rejected task was not kept");
        assert_eq!(by_lead["dependsOn"], serde_json::json!([first.id.0]));
        assert_eq!(
            lock(backend.storage()).list_relations().unwrap(),
            [(model::TaskId(agent_id), model::TaskId(id))]
        );
        let links = format!(
            "; it depends on #{} and is related to #{agent_id}.",
            first.id.0
        );
        assert_eq!(
            last_note(&by_lead),
            format!("Created by the Toodo lead{links}")
        );
        assert_eq!(
            chat(&backend),
            [format!("Created #{id} Add sessions{links}")]
        );
        assert_eq!(*backend.chat_changed.lock().unwrap(), 1);
    }

    #[test]
    fn the_lead_starts_a_task_with_its_agent_in_the_new_pane() {
        let (storage, task) = storage_with_task(Some(r"D:\wt"));
        let created =
            r#"{"id":"cli:worktree:create","result":{"workspace":{"workspace_id":"w27"}}}"#;
        let panes = r#"{"id":"cli:pane:list","result":{"panes":[
            {"pane_id":"w9:p1","workspace_id":"w9","agent_status":"idle"},
            {"pane_id":"w27:p1","workspace_id":"w27","agent_status":"unknown"}]}}"#;
        let backend = Arc::new(TestBackend::scripted(
            storage,
            vec![
                Ok("abc\n".to_owned()),
                Ok(String::new()),
                Err(crate::adapters::git::fake::failed()),
                Ok(String::new()),
            ],
            vec![
                Ok(created.to_owned()),
                Ok(panes.to_owned()),
                Ok(String::new()),
            ],
        ));
        let server = ToodoMcp::new(backend.clone());
        let request = tools::StartTask {
            id: task.id.into(),
            agent_command: Some(" omo --approve ".to_owned()),
            request_id: Some("chat-1/start-1".to_owned()),
        };

        let result = runtime()
            .block_on(server.start_task(by(lead::Caller::Lead), Parameters(request.clone())))
            .unwrap();

        let started = created_workspace(&serde_json::to_value(result).unwrap());
        assert_eq!(started["status"], "in_progress");
        assert_eq!(started["ranIn"], "w27:p1");
        assert_eq!(started["panes"].as_array().unwrap().len(), 1);
        assert_eq!(started["panes"][0]["paneId"], "w27:p1");
        let ran = " and ran `omo --approve` in its pane w27:p1.";
        assert_eq!(
            last_note(&started),
            format!("Started by the Toodo lead{ran}")
        );
        assert_eq!(
            chat(&backend),
            [format!("Started #{} Add login{ran}", task.id.0)]
        );

        // The same request again, e.g. by a lead that restarted: no git, herdr, or chat.
        let again = runtime()
            .block_on(server.start_task(by(lead::Caller::Lead), Parameters(request)))
            .unwrap();
        let again = created_workspace(&serde_json::to_value(again).unwrap());
        assert_eq!(
            (&again["id"], &again["status"]),
            (&started["id"], &started["status"])
        );
        assert_eq!(again.get("ranIn"), None);
        assert_eq!(chat(&backend).len(), 1);
        let create = tools::CreateTask {
            workspace_id: task.workspace_id.into(),
            task_type: crate::commands::types::TaskType::Feat,
            title: "Other".to_owned(),
            description: String::new(),
            depends_on: Vec::new(),
            related: Vec::new(),
            estimate: None,
            due_date: None,
            criteria: Vec::new(),
            merge_criterion: Some(false),
            request_id: Some("chat-1/start-1".to_owned()),
        };
        let reused = runtime()
            .block_on(server.create_task(by(lead::Caller::Lead), Parameters(create)))
            .unwrap();
        let reused = serde_json::to_value(reused).unwrap();
        assert!(error_text(&reused).contains("already named"), "{reused}");
    }

    #[test]
    fn a_create_named_by_a_request_id_runs_once() {
        let (storage, first) = storage_with_task(None);
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let create = |request_id: &str| {
            let request = serde_json::from_value(serde_json::json!({
                "workspace_id": first.workspace_id.0,
                "task_type": "feat",
                "title": "Add logout",
                "merge_criterion": false,
                "request_id": request_id,
            }))
            .unwrap();
            let result = runtime()
                .block_on(server.create_task(by(lead::Caller::Lead), Parameters(request)))
                .unwrap();
            serde_json::to_value(result).unwrap()
        };

        let created = created_workspace(&create("chat-3/create-1"));
        let again = created_workspace(&create(" chat-3/create-1 "));
        let other = created_workspace(&create("chat-3/create-2"));

        assert_eq!(again["id"], created["id"]);
        assert_ne!(other["id"], created["id"]);
        assert_eq!(
            lock(backend.storage())
                .list_tasks(first.workspace_id)
                .unwrap()
                .len(),
            3
        );
        assert_eq!(chat(&backend).len(), 2, "one line per created task");
        assert!(error_text(&create(" ")).starts_with("InvalidInput:"));
    }

    #[test]
    fn only_the_lead_takes_its_inbox_and_a_new_session_gets_what_waits() {
        let (mut storage, _) = storage_with_task(None);
        let message = storage
            .add_chat_message(crate::domain::chat::ChatAuthor::User, "Plan it", Some("c1"))
            .unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let take = |caller, arguments: serde_json::Value| {
            let request = serde_json::from_value(arguments).unwrap();
            serde_json::to_value(
                server
                    .take_pending(by(caller), Parameters(request))
                    .unwrap(),
            )
            .unwrap()
        };
        let chat_id = format!("chat-{}", message.id);

        let refused = take(lead::Caller::Agent, serde_json::json!({}));
        assert!(error_text(&refused).starts_with("Rejected:"));
        // A lead session that started after the message was stored takes it.
        let taken = created_workspace(&take(lead::Caller::Lead, serde_json::json!({})));
        assert_eq!(
            taken,
            serde_json::json!({
                "items": [{ "id": chat_id, "at": message.at, "kind": "chat", "body": "Plan it" }],
            })
        );
        assert_eq!(
            *backend.chat_changed.lock().unwrap(),
            1,
            "seen in the panel"
        );
        let none = created_workspace(&take(lead::Caller::Lead, serde_json::json!({})));
        assert_eq!(none["items"], serde_json::json!([]));
        let by_id = created_workspace(&take(
            lead::Caller::Lead,
            serde_json::json!({ "ids": [chat_id, "event-77"] }),
        ));
        assert!(by_id["items"][0]["takenBefore"].is_number(), "{by_id}");
        assert_eq!(by_id["missing"], serde_json::json!(["event-77"]));
        let bad = take(lead::Caller::Lead, serde_json::json!({ "ids": ["task-1"] }));
        assert!(error_text(&bad).starts_with("InvalidInput:"));
    }

    fn conclude(
        server: &ToodoMcp,
        caller: lead::Caller,
        id: model::TaskId,
        confirmation: Option<&str>,
    ) -> serde_json::Value {
        let request = tools::ConcludeTask {
            id: id.into(),
            note: "Duplicate of #7\nSee there.".to_owned(),
            confirmation: confirmation.map(str::to_owned),
        };
        let result = runtime()
            .block_on(server.conclude_task(by(caller), Parameters(request)))
            .unwrap();
        serde_json::to_value(result).unwrap()
    }

    #[test]
    fn the_lead_concludes_a_task_only_once_a_person_confirms_it_in_the_chat() {
        use crate::domain::chat::ChatAuthor;
        use crate::domain::status::TaskStatus;
        let (mut storage, task) = storage_with_task(None);
        storage
            .set_task_status(task.id, TaskStatus::InProgress)
            .unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let status = || {
            lock(backend.storage())
                .get_task(task.id)
                .unwrap()
                .unwrap()
                .status
        };

        let asked = error_text(&conclude(&server, lead::Caller::Lead, task.id, None)).to_owned();
        let code = asked.split("with the code ").nth(1).unwrap()[..6].to_owned();
        assert_eq!(status(), TaskStatus::InProgress);
        let request = chat(&backend).pop().unwrap();
        assert!(
            request.contains("> Duplicate of #7\n> See there."),
            "{request}"
        );
        assert!(
            request.contains(&format!("Reply `confirm {code}`")),
            "{request}"
        );

        let waiting = conclude(&server, lead::Caller::Lead, task.id, Some(&code));
        assert!(error_text(&waiting).contains("yet"));
        lock(backend.storage())
            .add_chat_message(ChatAuthor::User, &format!("confirm {code}"), None)
            .unwrap();
        let done = created_workspace(&conclude(&server, lead::Caller::Lead, task.id, Some(&code)));

        assert_eq!(
            (&done["status"], &done["resolution"]),
            (
                &serde_json::json!("done"),
                &serde_json::json!("not_adopted")
            )
        );
        let confirmed = format!(", confirmed in the chat (`confirm {code}`).");
        assert_eq!(
            last_note(&done),
            format!("Concluded without a merge by the Toodo lead{confirmed}")
        );
        // The server reports the finish; the lead's call adds no line of its own.
        let messages = chat(&backend);
        assert_eq!(messages.len(), 3, "{messages:?}");
        assert_eq!(
            messages[2],
            format!(
                "**Not adopted:** #{} Add login was concluded without a merge.\n\n> Duplicate of #7",
                task.id.0
            )
        );
        assert_eq!(*backend.chat_changed.lock().unwrap(), 2);
    }

    #[test]
    fn other_agents_conclude_without_a_confirmation_and_the_server_reports_it() {
        let (mut storage, task) = storage_with_task(None);
        storage
            .set_task_status(task.id, crate::domain::status::TaskStatus::InProgress)
            .unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());

        let done = created_workspace(&conclude(&server, lead::Caller::Agent, task.id, None));

        assert_eq!(done["status"], "done");
        assert_eq!(done["notes"], serde_json::json!([]));
        let messages = chat(&backend);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].starts_with("**Not adopted:**"), "{messages:?}");
        assert_eq!(*backend.chat_changed.lock().unwrap(), 1);
    }

    #[test]
    fn the_lead_asks_before_confirming_a_completion_or_completing_with_another_task() {
        use crate::domain::status::TaskStatus;
        let (mut storage, task) = storage_with_task(None);
        storage
            .set_task_status(task.id, TaskStatus::InProgress)
            .unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let complete = |merged_with: Option<tools::MergedWithRef>| {
            let request = tools::CompleteTask {
                id: task.id.into(),
                confirmation: None,
                merged_with,
            };
            let result = runtime()
                .block_on(server.complete_task(by(lead::Caller::Lead), Parameters(request)))
                .unwrap();
            serde_json::to_value(result).unwrap()
        };

        let shipped = complete(Some(tools::MergedWithRef::Text(" #5 ".to_owned())));
        assert!(error_text(&shipped).contains("confirm"));
        lock(backend.storage())
            .set_task_status(task.id, TaskStatus::InReview)
            .unwrap();
        let confirming = complete(None);
        assert!(error_text(&confirming).contains("confirm"));

        let requests = chat(&backend);
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[0].contains("complete #1 Add login as shipped with #5,"));
        assert!(requests[1].contains("awaits a person's confirmation"));
        let stored = lock(backend.storage()).get_task(task.id).unwrap().unwrap();
        assert_eq!(stored.status, TaskStatus::InReview);
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
            vec![
                Ok(created.to_owned()),
                Ok(r#"{"id":"cli:pane:list","result":{"panes":[]}}"#.to_owned()),
            ],
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

    fn memory_call(
        server: &ToodoMcp,
        tool: &str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        fn args<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
            serde_json::from_value(value).unwrap()
        }
        let result = match tool {
            "list_memory" => server.list_memory(Parameters(args(arguments))),
            "search_memory" => server.search_memory(Parameters(args(arguments))),
            "get_memory" => server.get_memory(Parameters(args(arguments))),
            "add_memory" => server.add_memory(Parameters(args(arguments))),
            "update_memory" => server.update_memory(Parameters(args(arguments))),
            "supersede_memory" => server.supersede_memory(Parameters(args(arguments))),
            _ => panic!("unknown tool {tool}"),
        };
        serde_json::to_value(result.unwrap()).unwrap()
    }

    fn memory_titles(result: &serde_json::Value) -> Vec<String> {
        created_workspace(result)
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["title"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn memory_tools_keep_workspace_and_shared_entries_and_their_history() {
        let (storage, task) = storage_with_task(None);
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let workspace = task.workspace_id.0;
        let call = |tool, arguments| memory_call(&server, tool, arguments);

        let shared = created_workspace(&call(
            "add_memory",
            serde_json::json!({ "kind": "rule", "title": "Write docs in English" }),
        ));
        assert_eq!(shared["workspaceId"], serde_json::Value::Null);
        let decision = created_workspace(&call(
            "add_memory",
            serde_json::json!({
                "workspace_id": workspace, "kind": "decision", "title": "Use make",
                "body": "Simple.", "task_id": task.id.0
            }),
        ));
        assert_eq!(decision["taskId"], task.id.0);
        let replaced = created_workspace(&call(
            "supersede_memory",
            serde_json::json!({
                "id": decision["id"], "title": "Use npm scripts", "body": "Runs on Windows."
            }),
        ));
        assert_eq!(
            (
                &replaced["kind"],
                &replaced["taskId"],
                &replaced["workspaceId"]
            ),
            (
                &serde_json::json!("decision"),
                &serde_json::json!(task.id.0),
                &serde_json::json!(workspace)
            )
        );

        let list = |arguments| memory_titles(&call("list_memory", arguments));
        assert_eq!(
            list(serde_json::json!({ "workspace_id": workspace })),
            ["Use npm scripts"]
        );
        assert_eq!(
            list(serde_json::json!({ "workspace_id": workspace, "include_shared": true })),
            ["Use npm scripts", "Write docs in English"]
        );
        assert_eq!(list(serde_json::json!({})), ["Write docs in English"]);
        assert_eq!(
            list(serde_json::json!({
                "workspace_id": workspace, "include_superseded": true, "kind": "decision"
            })),
            ["Use npm scripts", "Use make"]
        );
        assert_eq!(
            memory_titles(&call(
                "search_memory",
                serde_json::json!({
                    "query": "windows NPM", "workspace_id": workspace, "include_shared": true
                }),
            )),
            ["Use npm scripts"]
        );

        let details = created_workspace(&call(
            "get_memory",
            serde_json::json!({ "id": replaced["id"] }),
        ));
        assert_eq!(details["workspace"], "toodo");
        assert_eq!(details["supersedes"][0]["title"], "Use make");
        assert_eq!(details["supersedes"][0]["supersededBy"], replaced["id"]);

        let fixed = created_workspace(&call(
            "update_memory",
            serde_json::json!({ "id": replaced["id"], "title": "Use npm scripts only", "task_id": null }),
        ));
        assert_eq!(
            (&fixed["body"], &fixed["taskId"]),
            (
                &serde_json::json!("Runs on Windows."),
                &serde_json::Value::Null
            )
        );
        assert_eq!(*backend.memory_changed.lock().unwrap(), 4);

        let old = error_text(&call(
            "update_memory",
            serde_json::json!({ "id": decision["id"], "title": "Again" }),
        ))
        .to_owned();
        assert!(old.starts_with("Rejected:"), "{old}");
        let blank = error_text(&call(
            "add_memory",
            serde_json::json!({ "kind": "note", "title": " " }),
        ))
        .to_owned();
        assert!(blank.starts_with("InvalidInput:"), "{blank}");
        let missing = error_text(&call(
            "list_memory",
            serde_json::json!({ "workspace_id": 99 }),
        ))
        .to_owned();
        assert!(missing.starts_with("NotFound:"), "{missing}");
        assert_eq!(*backend.memory_changed.lock().unwrap(), 4);
    }

    #[test]
    fn a_chat_reply_is_stored_and_announced() {
        let backend = Arc::new(TestBackend::new(
            crate::storage::Storage::open_in_memory().unwrap(),
        ));
        let server = ToodoMcp::new(backend.clone());
        let call = |text: &str| {
            serde_json::to_value(
                server
                    .reply_in_chat(
                        by(lead::Caller::Agent),
                        Parameters(tools::ChatReply {
                            text: text.to_owned(),
                            handles: None,
                        }),
                    )
                    .unwrap(),
            )
            .unwrap()
        };

        assert!(error_text(&call(" ")).contains("empty"));
        let posted = created_workspace(&call("Created **#4**."));

        assert_eq!(
            (&posted["author"], &posted["body"]),
            (
                &serde_json::json!("lead"),
                &serde_json::json!("Created **#4**.")
            )
        );
        assert_eq!(*backend.chat_changed.lock().unwrap(), 1);
        assert_eq!(
            lock(backend.storage()).list_chat_messages().unwrap().len(),
            1
        );
    }

    #[test]
    fn only_the_lead_acknowledges_and_marks_messages_handled() {
        let mut storage = crate::storage::Storage::open_in_memory().unwrap();
        let message = storage
            .add_chat_message(crate::domain::chat::ChatAuthor::User, "Plan it", None)
            .unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let server = ToodoMcp::new(backend.clone());
        let id = format!("chat-{}", message.id);
        let value = |result: Result<CallToolResult, ErrorData>| {
            serde_json::to_value(result.unwrap()).unwrap()
        };
        let ack = |caller| {
            let request = serde_json::from_value(serde_json::json!({ "ids": id })).unwrap();
            value(server.ack_chat(by(caller), Parameters(request)))
        };
        let answer = |caller| {
            let request = serde_json::from_value(
                serde_json::json!({ "text": "Planned #4.", "handles": [id] }),
            )
            .unwrap();
            value(server.reply_in_chat(by(caller), Parameters(request)))
        };
        let mark = |caller, arguments: serde_json::Value| {
            let request = serde_json::from_value(arguments).unwrap();
            value(server.mark_chat_handled(by(caller), Parameters(request)))
        };

        assert!(error_text(&ack(lead::Caller::Agent)).starts_with("Rejected:"));
        assert!(error_text(&answer(lead::Caller::Agent)).starts_with("Rejected:"));
        let reply_id = serde_json::json!({ "id": id, "reply_id": 1 });
        assert!(error_text(&mark(lead::Caller::Agent, reply_id)).starts_with("Rejected:"));
        assert_eq!(*backend.chat_changed.lock().unwrap(), 0, "nothing changed");
        let stored = lock(backend.storage()).list_chat_messages().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!((stored[0].acked_at, stored[0].handled_at), (None, None));

        let acked = created_workspace(&ack(lead::Caller::Lead));
        assert!(acked[0]["ackedAt"].is_number(), "{acked}");
        let posted = created_workspace(&answer(lead::Caller::Lead));
        let both = serde_json::json!({ "id": id, "reply_id": posted["id"], "task_id": 1 });
        assert!(error_text(&mark(lead::Caller::Lead, both)).starts_with("InvalidInput:"));
        let again = serde_json::json!({ "id": id, "reply_id": posted["id"] });
        let handled = created_workspace(&mark(lead::Caller::Lead, again));
        assert_eq!(
            handled["handledBy"],
            serde_json::json!({ "kind": "reply", "messageId": posted["id"] })
        );
        assert_eq!(handled["overdue"], false);
        assert_eq!(*backend.chat_changed.lock().unwrap(), 3);
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
                criteria: Vec::new(),
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
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
        post(address, authorization, body)
    }

    fn post(address: SocketAddr, authorization: &str, body: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n{authorization}\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        // A server that answers without reading the body closes with a reset, which macOS
        // reports after the answer; the answer read up to then is the response.
        let mut response = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => response.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(error) => panic!("reading the response failed: {error}"),
            }
        }
        String::from_utf8(response).unwrap()
    }

    #[test]
    fn only_requests_with_the_token_reach_the_tools() {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let address = listener.local_addr().unwrap();
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let app = router(backend, "secret".to_owned(), Some("leader".to_owned()));
        std::thread::spawn(move || runtime().block_on(serve(listener, app)));

        assert!(post_initialize(address, "").starts_with("HTTP/1.1 401"));
        let wrong = post_initialize(address, "Authorization: Bearer secreT\r\n");
        assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
        let accepted = post_initialize(address, "Authorization: Bearer secret\r\n");
        assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
        assert!(accepted.contains(r#""name":"toodo""#), "{accepted}");
        let lead = post_initialize(address, "Authorization: Bearer leader\r\n");
        assert!(lead.starts_with("HTTP/1.1 200"), "{lead}");
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
            "list_memory",
            "search_memory",
            "get_memory",
            "add_memory",
            "update_memory",
            "supersede_memory",
            "reply_in_chat",
            "take_pending",
            "ack_chat",
            "mark_chat_handled",
        ] {
            assert!(server.tool_router.has_route(tool), "{tool}");
        }
        assert!(!server.tool_router.has_route("delete_task"));
        let skill =
            |caller| serde_json::to_value(server.read_toodo_skill(by(caller)).unwrap()).unwrap();
        let result = skill(lead::Caller::Agent);
        assert_eq!(result["content"][0]["text"], SKILL);
        // `lines` also drops the `\r` of a CRLF checkout on Windows.
        assert_eq!(
            SKILL.lines().take(2).collect::<Vec<_>>(),
            ["---", "name: toodo"]
        );
        assert_ne!(result["isError"], true);
    }

    #[test]
    fn the_lead_token_reads_the_lead_skill_and_the_agent_token_the_worker_skill() {
        assert_eq!(
            LEAD_SKILL.lines().take(2).collect::<Vec<_>>(),
            ["---", "name: toodo-lead"]
        );
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let address = listener.local_addr().unwrap();
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let app = router(backend, "secret".to_owned(), Some("leader".to_owned()));
        std::thread::spawn(move || runtime().block_on(serve(listener, app)));
        let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_toodo_skill","arguments":{}}}"#;
        // The skill text as it appears in the JSON reply.
        let encoded = |skill: &str| {
            let json = serde_json::to_string(skill).unwrap();
            json[1..json.len() - 1].to_owned()
        };

        let lead = post(address, "Authorization: Bearer leader\r\n", call);
        assert!(lead.starts_with("HTTP/1.1 200"), "{lead}");
        assert!(lead.contains(&encoded(LEAD_SKILL)), "{lead}");
        let agent = post(address, "Authorization: Bearer secret\r\n", call);
        assert!(agent.starts_with("HTTP/1.1 200"), "{agent}");
        assert!(agent.contains(&encoded(SKILL)), "{agent}");
        assert!(!agent.contains("name: toodo-lead"), "{agent}");
    }

    #[test]
    fn a_call_without_a_caller_reads_the_worker_skill() {
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let server = ToodoMcp::new(Arc::new(TestBackend::new(storage)));
        let parts = axum::http::Request::new(()).into_parts().0;
        let result = serde_json::to_value(server.read_toodo_skill(Extension(parts)).unwrap());
        assert_eq!(result.unwrap()["content"][0]["text"], SKILL);
    }

    #[test]
    fn the_lead_token_reads_the_lead_skill_after_a_restart_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lead-token");
        let token = load_token(&file).unwrap();
        // A restarted server reads the same file, and the lead keeps the token it started
        // with.
        let reloaded = load_token(&file).unwrap();
        assert_eq!(reloaded, token);
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let address = listener.local_addr().unwrap();
        let storage = crate::storage::Storage::open_in_memory().unwrap();
        let backend = Arc::new(TestBackend::new(storage));
        let app = router(backend, "secret".to_owned(), Some(reloaded));
        std::thread::spawn(move || runtime().block_on(serve(listener, app)));
        let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_toodo_skill","arguments":{}}}"#;

        let lead = post(address, &format!("Authorization: Bearer {token}\r\n"), call);
        assert!(lead.starts_with("HTTP/1.1 200"), "{lead}");
        assert!(lead.contains("name: toodo-lead"), "{lead}");
    }

    #[test]
    fn secrets_match_only_when_equal() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abd", "abc"));
        assert!(!same_secret("ab", "abc"));
    }
}
