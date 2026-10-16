//! MCP server for coding agents, embedded in the app so it shares the database lock and
//! tells the frontend about changes like any command.
//!
//! It serves Streamable HTTP on a fixed loopback port and requires the bearer token kept
//! in the app data directory. Agents can read workspaces and tasks, create and edit
//! tasks, connect dependencies, leave progress and blocker notes in a task's history, and
//! start, complete, or conclude tasks through the app's own commands; deleting tasks stays
//! in the app. The `toodo` skill, the
//! workflow for working on a task, is served by the `read_toodo_skill` tool rather than
//! as a resource, since not every client can read resources. Agents on SSH machines reach
//! the server through the reverse tunnels of [`tunnels`].

mod tools;
pub mod tunnels;

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
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
use tauri::{AppHandle, Manager};

use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::commands::events::Notifier;
use crate::commands::types::Task;
use crate::commands::{CommandResult, GitHubState, GitState, HerdrState, StorageState, lock};
use crate::domain::model;

pub const PORT: u16 = 47615;

const TOKEN_FILE: &str = "mcp-token";

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
request. If it finds no task, use list_workspaces to pick the workspace.
2. Before create_task, call list_tasks for that workspace and reuse a matching task \
instead of creating a duplicate.
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

Deleting tasks happens in the Toodo app only, not through these tools. A pull request \
from the task branch into baseBranch moves the task to in_review, and its merge \
completes the task; never merge it yourself.";

/// What the app needs to serve the tools: the database, the adapters the app's commands
/// run, and a way to announce changes.
pub trait Backend: Notifier + Send + Sync + 'static {
    fn storage(&self) -> &StorageState;
    fn git(&self) -> &Git;
    fn github(&self) -> &GitHub;
    fn herdr(&self) -> &Herdr;
}

impl Backend for AppHandle {
    fn storage(&self) -> &StorageState {
        self.state::<StorageState>().inner()
    }

    fn git(&self) -> &Git {
        self.state::<GitState>().inner()
    }

    fn github(&self) -> &GitHub {
        self.state::<GitHubState>().inner()
    }

    fn herdr(&self) -> &Herdr {
        self.state::<HerdrState>().inner()
    }
}

/// How agents reach the server, shown in the app; `error` tells why it is not running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub url: String,
    pub token: Option<String>,
    pub error: Option<String>,
}

/// Starts the server in the background. Failures, e.g. the port being taken, are
/// reported in the status rather than stopping the app.
pub fn start(backend: Arc<dyn Backend>, data_dir: &Path) -> ServerStatus {
    let url = format!("http://127.0.0.1:{PORT}/mcp");
    let token = match load_token(&data_dir.join(TOKEN_FILE)) {
        Ok(token) => token,
        Err(error) => {
            return ServerStatus {
                url,
                token: None,
                error: Some(format!(
                    "could not read or create the access token: {error}"
                )),
            };
        }
    };
    let listener = match bind(SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))) {
        Ok(listener) => listener,
        Err(error) => {
            return ServerStatus {
                url,
                token: Some(token),
                error: Some(format!("could not listen on port {PORT}: {error}")),
            };
        }
    };
    let app = router(backend, token.clone());
    tauri::async_runtime::spawn(async move {
        if let Err(error) = serve(listener, app).await {
            eprintln!("toodo: MCP server stopped: {error}");
        }
    });
    ServerStatus {
        url,
        token: Some(token),
        error: None,
    }
}

fn bind(address: SocketAddr) -> io::Result<TcpListener> {
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

async fn serve(listener: TcpListener, app: axum::Router) -> io::Result<()> {
    axum::serve(tokio::net::TcpListener::from_std(listener)?, app).await
}

/// The token in `path`, created with 32 random bytes on first use.
fn load_token(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    fs::write(path, &token)?;
    Ok(token)
}

fn router(backend: Arc<dyn Backend>, token: String) -> axum::Router {
    let service = StreamableHttpService::new(
        move || Ok(ToodoMcp::new(backend.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::new(token),
            require_token,
        ))
}

async fn require_token(State(token): State<Arc<String>>, request: Request, next: Next) -> Response {
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
            "missing or wrong Toodo MCP access token",
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
        let backend = self.backend.clone();
        let result = tauri::async_runtime::spawn_blocking(move || apply(backend.as_ref()))
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
        self.announce(result)
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
        description = "List tasks with status and depends_on, of one workspace or of all workspaces."
    )]
    fn list_tasks(
        &self,
        Parameters(request): Parameters<tools::ListTasks>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(tools::list_tasks(&lock(self.backend.storage()), request))
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
        description = "Start a planned task whose prerequisites are all done, as Start (or Resume for a paused task) in the Toodo app does: create or reopen its worktree and herdr workspace and move it to in_progress. Use it to hand a ready task to a new agent session; never start your own task, the one current_task finds. Rejected with the reason when the task is not ready, is not planned, or its machine has no worktree root."
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

    struct TestBackend {
        storage: StorageState,
        git: Git,
        github: GitHub,
        herdr: Herdr,
        changed: std::sync::Mutex<Vec<model::WorkspaceId>>,
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
            }
        }
    }

    impl Notifier for TestBackend {
        fn tasks_changed(&self, workspace: model::WorkspaceId) {
            self.changed.lock().unwrap().push(workspace);
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
                    path: WindowsPath::parse(r"C:\dev\toodo").unwrap(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        storage
            .save_machine_settings(&MachineSettings::Local {
                worktree_root: root.map(|root| WindowsPath::parse(root).unwrap()),
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
        let result =
            tauri::async_runtime::block_on(server.start_task(Parameters(request))).unwrap();
        serde_json::to_value(result).unwrap()
    }

    #[test]
    fn starting_a_ready_task_moves_it_in_progress_and_announces_it() {
        let (storage, task) = storage_with_task(Some(r"D:\wt"));
        let created =
            r#"{"id":"cli:worktree:create","result":{"workspace":{"workspace_id":"w27"}}}"#;
        let backend = Arc::new(TestBackend::scripted(
            storage,
            vec![Ok("abc\n".to_owned())],
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
            "InvalidInput: worktree root is not set for machine local"
        );
        assert!(backend.changed.lock().unwrap().is_empty());
        let stored = lock(&backend.storage).get_task(task.id).unwrap().unwrap();
        assert_eq!(stored, task);
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
        tauri::async_runtime::spawn(serve(listener, router(backend, "secret".to_owned())));

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
        for tool in ["start_task", "complete_task", "conclude_task"] {
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
