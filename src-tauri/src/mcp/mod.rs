//! MCP server for coding agents, embedded in the app so it shares the database lock and
//! tells the frontend about changes like any command.
//!
//! It serves Streamable HTTP on a fixed loopback port and requires the bearer token kept
//! in the app data directory. Agents can read workspaces and tasks, create and edit
//! tasks, connect dependencies, and leave progress and blocker notes in a task's history;
//! starting, completing, and deleting tasks stay in the app. Agents on SSH machines reach it through the reverse tunnels of [`tunnels`].

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

use crate::adapters::herdr::Herdr;
use crate::commands::events::Notifier;
use crate::commands::types::Task;
use crate::commands::{CommandResult, HerdrState, StorageState, lock};
use crate::domain::model;

pub const PORT: u16 = 47615;

const TOKEN_FILE: &str = "mcp-token";

const INSTRUCTIONS: &str = "Toodo plans tasks as a dependency DAG across workspaces; \
a task becomes ready only when all tasks it depends on are done.

How to use these tools:
1. Call current_task with your absolute working directory first. Use its workspace_id \
for new tasks in the same repository. If it finds no task, use list_workspaces to pick \
the workspace.
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
   Adding a prerequisite that is not done to an in_progress task pauses it and closes \
its herdr workspace, which ends your session; the user resumes the task in the Toodo \
app once its prerequisites are done. So create every task first, tell the user what \
you planned and why, and make that add_dependency call last, with all prerequisites \
in one call. Use remove_dependency to drop an edge.
4. Pick task_type by the kind of change (feat, fix, modify, refactor, docs, test, chore, \
rnd), write a short imperative title, and put details in description. Write the \
description in GitHub-flavored Markdown (Toodo renders it and uses it as the GitHub \
issue body): short paragraphs, `-` lists, `- [ ]` task lists for acceptance criteria, \
backticks for code, paths, and commands, fenced code blocks, and links; no raw HTML.
5. Leave a record of your work on your task as you go; it feeds the user's reviews of \
their work. Call add_progress_note when you take a decision worth remembering (the choice \
and why, e.g. a library or approach over its alternatives) or finish a meaningful step, \
and report_blocker when something stops you that needs the user or someone else (missing \
access, an unclear requirement, a failing dependency), saying what you need. Write one \
or two Markdown sentences per note; skip routine steps such as running tests or small \
edits. Neither tool changes the task's status.
6. A tool error says what was rejected; change the arguments instead of retrying them \
unchanged.

Starting, completing, and deleting tasks happen in the Toodo app, not through these \
tools.";

/// What the app needs to serve the tools: the database and a way to announce changes.
pub trait Backend: Notifier + Send + Sync + 'static {
    fn storage(&self) -> &StorageState;
    /// For closing the herdr workspace of a task that a new prerequisite pauses.
    fn herdr(&self) -> &Herdr;
}

impl Backend for AppHandle {
    fn storage(&self) -> &StorageState {
        self.state::<StorageState>().inner()
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

    fn announce(&self, result: CommandResult<Task>) -> Result<CallToolResult, ErrorData> {
        if let Ok(task) = &result {
            self.backend
                .tasks_changed(model::WorkspaceId(task.workspace_id.0));
        }
        reply(result)
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

    #[tool(description = "Change the type, title, or description of a task.")]
    fn update_task(
        &self,
        Parameters(request): Parameters<tools::UpdateTask>,
    ) -> Result<CallToolResult, ErrorData> {
        self.change(|storage| tools::update_task(storage, request))
    }

    #[tool(
        description = "Make task_id depend on every task in depends_on. Rejects self and cyclic dependencies; one rejected prerequisite adds none. If task_id is in_progress and a prerequisite is not done, the task is paused and its herdr workspace closed."
    )]
    fn add_dependency(
        &self,
        Parameters(request): Parameters<tools::AddDependencies>,
    ) -> Result<CallToolResult, ErrorData> {
        // A paused task's herdr workspace is closed, so the lock is taken inside.
        self.announce(tools::add_dependencies(
            self.backend.storage(),
            self.backend.herdr(),
            request,
        ))
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

    struct TestBackend(StorageState, Herdr);

    impl Notifier for TestBackend {
        fn tasks_changed(&self, _: model::WorkspaceId) {}
        fn agent_statuses_changed(&self, _: &[crate::commands::types::TaskAgentStatus]) {}
    }

    impl Backend for TestBackend {
        fn storage(&self) -> &StorageState {
            &self.0
        }

        fn herdr(&self) -> &Herdr {
            &self.1
        }
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
        let herdr = Herdr::new(Box::new(Arc::new(
            crate::adapters::git::fake::FakeRunner::new([]),
        )));
        let backend = Arc::new(TestBackend(std::sync::Mutex::new(storage), herdr));
        tauri::async_runtime::spawn(serve(listener, router(backend, "secret".to_owned())));

        assert!(post_initialize(address, "").starts_with("HTTP/1.1 401"));
        let wrong = post_initialize(address, "Authorization: Bearer secreT\r\n");
        assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");
        let accepted = post_initialize(address, "Authorization: Bearer secret\r\n");
        assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
        assert!(accepted.contains(r#""name":"toodo""#), "{accepted}");
    }

    #[test]
    fn secrets_match_only_when_equal() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abd", "abc"));
        assert!(!same_secret("ab", "abc"));
    }
}
