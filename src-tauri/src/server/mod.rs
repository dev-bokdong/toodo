//! The headless server (`toodo-server`) for a Linux machine that runs herdr itself: the
//! commands of the desktop app over HTTP for the web build of the frontend, the events the
//! desktop app emits as a server-sent event stream, and the MCP server for agents, all on
//! one port. Workspaces on "Local" are on the server, with POSIX paths.
//!
//! - `POST /api/rpc/<command>` runs a command (see [`rpc`]).
//! - `GET /api/events` streams `tasks-changed`, `agent-statuses-changed`, and
//!   `pull-request-statuses-changed` events with JSON data.
//! - `/mcp` is the MCP endpoint, as in the desktop app.
//! - Anything else is served from the web build of the frontend, when one is configured.
//!
//! `/api` requires the web access token and `/mcp` the MCP access token as a bearer token;
//! both are created in the data directory on first start.

mod rpc;

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::middleware;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures_util::Stream;
use tokio::sync::broadcast;
use tower_http::services::{ServeDir, ServeFile};

use crate::adapters::git::{Git, ProcessRunner};
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::adapters::ssh::{Ssh, SshConfigFile};
use crate::adapters::ssh_tunnel::SshForwarder;
use crate::commands::backup::{self, Backups};
use crate::commands::completion::{self, PullRequestStatusState};
use crate::commands::events::{
    self, AgentStatusState, AgentStatusesChanged, Notifier, PullRequestStatusesChanged,
    TasksChanged, names,
};
use crate::commands::types::{TaskAgentStatus, TaskPullRequestStatus};
use crate::commands::{
    GitHubState, GitState, HerdrState, Runtime, SshConfigState, SshState, StorageState,
};
use crate::domain::model;
use crate::mcp::tunnels::{self, Tunnels, TunnelsState};
use crate::mcp::{self, Backend, ServerStatus};
use crate::storage::Storage;

const DATABASE_FILE: &str = "toodo.sqlite3";

const WEB_TOKEN_FILE: &str = "web-token";

/// The default port, next to the desktop app's MCP port ([`mcp::PORT`]) rather than on it:
/// the desktop app forwards that port to its SSH machines, which may include this server,
/// and the server forwards its own port to its SSH machines the same way, so the two must
/// differ to share a machine.
pub const DEFAULT_PORT: u16 = mcp::PORT + 1;

/// Events kept for subscribers that read slower than they arrive; a subscriber that falls
/// further behind skips the oldest.
const EVENT_BUFFER: usize = 64;

const USAGE: &str = "Usage: toodo-server

Serves Toodo on one port: the web app, its API, and the MCP server for agents.

Environment:
  TOODO_LISTEN    address to listen on (default 127.0.0.1:47616)
  TOODO_DATA_DIR  database, backups, and access tokens
                  (default $XDG_DATA_HOME/toodo-server or ~/.local/share/toodo-server)
  TOODO_WEB_DIR   web build of the frontend (`npm run build:web` writes dist-web);
                  without it only the API and MCP are served";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dir: Option<PathBuf>,
}

impl Config {
    /// Reads the configuration from `TOODO_*` environment variables, through `var`.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let listen = match var("TOODO_LISTEN") {
            Some(listen) => listen
                .parse()
                .map_err(|error| format!("TOODO_LISTEN {listen:?} is not an address: {error}"))?,
            None => SocketAddr::from((Ipv4Addr::LOCALHOST, DEFAULT_PORT)),
        };
        let data_dir = match var("TOODO_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => match (var("XDG_DATA_HOME"), var("HOME")) {
                (Some(data), _) if !data.is_empty() => Path::new(&data).join("toodo-server"),
                (_, Some(home)) => Path::new(&home).join(".local/share/toodo-server"),
                _ => return Err("set TOODO_DATA_DIR or HOME".to_owned()),
            },
        };
        let web_dir = var("TOODO_WEB_DIR").map(PathBuf::from);
        Ok(Self {
            listen,
            data_dir,
            web_dir,
        })
    }
}

/// An event for the frontend: its name and JSON data.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerEvent {
    name: &'static str,
    data: String,
}

/// Everything the commands, the MCP server, and the background threads use.
pub struct Services {
    storage: StorageState,
    git: GitState,
    github: GitHubState,
    ssh: SshState,
    ssh_config: SshConfigState,
    herdr: HerdrState,
    agent_statuses: AgentStatusState,
    pull_request_statuses: PullRequestStatusState,
    backups: Backups,
    mcp: ServerStatus,
    tunnels: TunnelsState,
    events: broadcast::Sender<ServerEvent>,
}

impl Services {
    fn emit(&self, name: &'static str, payload: &impl serde::Serialize) {
        match serde_json::to_string(payload) {
            // No subscriber is the usual case while no browser is open.
            Ok(data) => drop(self.events.send(ServerEvent { name, data })),
            Err(error) => eprintln!("toodo: failed to encode {name}: {error}"),
        }
    }
}

impl Notifier for Services {
    fn tasks_changed(&self, workspace: model::WorkspaceId) {
        let event = TasksChanged {
            workspace_id: workspace.into(),
        };
        self.emit(names::TASKS_CHANGED, &event);
    }

    fn agent_statuses_changed(&self, statuses: &[TaskAgentStatus]) {
        let event = AgentStatusesChanged {
            statuses: statuses.to_vec(),
        };
        self.emit(names::AGENT_STATUSES_CHANGED, &event);
    }

    fn pull_request_statuses_changed(&self, statuses: &[TaskPullRequestStatus]) {
        let event = PullRequestStatusesChanged {
            statuses: statuses.to_vec(),
        };
        self.emit(names::PULL_REQUEST_STATUSES_CHANGED, &event);
    }
}

impl Backend for Services {
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

impl Runtime for Services {
    fn agent_statuses(&self) -> &AgentStatusState {
        &self.agent_statuses
    }

    fn pull_request_statuses(&self) -> &PullRequestStatusState {
        &self.pull_request_statuses
    }

    fn backups(&self) -> &Backups {
        &self.backups
    }
}

/// Runs the server until it is interrupted (Ctrl+C or SIGTERM).
pub fn main() {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "-h" || arg == "--help")
    {
        println!("{USAGE}");
        return;
    }
    let config = match Config::from_env(|name| std::env::var(name).ok()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("toodo-server: {error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(error) = run(config) {
        eprintln!("toodo-server: {error}");
        std::process::exit(1);
    }
}

fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    create_private_dir(&config.data_dir)?;
    let storage = Storage::open(&config.data_dir.join(DATABASE_FILE))?;
    let mcp_token = mcp::load_token(&config.data_dir.join(mcp::TOKEN_FILE))?;
    let web_token_path = config.data_dir.join(WEB_TOKEN_FILE);
    let web_token = mcp::load_token(&web_token_path)?;
    let listener = mcp::bind(config.listen)?;
    let port = listener.local_addr()?.port();
    let services = Arc::new(Services {
        storage: Mutex::new(storage),
        git: Arc::new(Git::new(Box::new(ProcessRunner))),
        github: Arc::new(GitHub::new(Box::new(ProcessRunner))),
        ssh: Arc::new(Ssh::new(Box::new(ProcessRunner))),
        ssh_config: SshConfigFile::in_home(),
        herdr: Arc::new(Herdr::new(Box::new(ProcessRunner))),
        agent_statuses: Mutex::default(),
        pull_request_statuses: Mutex::default(),
        backups: Backups::load(&config.data_dir),
        mcp: ServerStatus {
            url: mcp::url(port),
            token: Some(mcp_token.clone()),
            error: None,
        },
        tunnels: Arc::new(Mutex::new(Tunnels::new(Box::new(SshForwarder), port))),
        events: broadcast::channel(EVENT_BUFFER).0,
    });
    events::spawn(services.clone())?;
    backup::spawn(services.clone())?;
    completion::spawn(services.clone())?;
    tunnels::spawn(services.clone(), services.tunnels.clone())?;

    let app = router(
        services.clone(),
        web_token,
        mcp_token,
        config.web_dir.as_deref(),
    );
    println!(
        "toodo-server: listening on http://{} (data in {}, web access token in {})",
        config.listen,
        config.data_dir.display(),
        web_token_path.display()
    );
    match &config.web_dir {
        Some(dir) => println!("toodo-server: serving the web app from {}", dir.display()),
        None => println!("toodo-server: TOODO_WEB_DIR is not set; serving the API and MCP only"),
    }
    let served = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            tokio::select! {
                served = mcp::serve(listener, app) => served,
                () = interrupted() => Ok(()),
            }
        });
    services
        .tunnels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .shut_down();
    Ok(served?)
}

/// Creates the data directory, which holds the access tokens, so that on Unix only the
/// owner can enter it; an existing one is tightened.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// The routes: the API behind the web token, MCP behind its own token, and the web app.
fn router(
    services: Arc<Services>,
    web_token: String,
    mcp_token: String,
    web_dir: Option<&Path>,
) -> Router {
    let api = Router::new()
        .route("/api/rpc/{command}", post(rpc::handle))
        .route("/api/events", get(event_stream))
        .layer(middleware::from_fn_with_state(
            Arc::new(web_token),
            mcp::require_token,
        ))
        .with_state(services.clone());
    let app = api.merge(mcp::router(services, mcp_token));
    match web_dir {
        Some(dir) => app
            .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")))),
        None => app,
    }
}

async fn event_stream(
    axum::extract::State(services): axum::extract::State<Arc<Services>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = futures_util::stream::unfold(services.events.subscribe(), |mut events| async {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let sent = Event::default().event(event.name).data(event.data);
                    return Some((Ok(sent), events));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

async fn interrupted() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("toodo-server: cannot listen for Ctrl+C: {error}");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => drop(signal.recv().await),
            Err(error) => {
                eprintln!("toodo-server: cannot listen for SIGTERM: {error}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    println!("toodo-server: stopping");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_port_is_not_the_desktop_apps_mcp_port() {
        let config =
            Config::from_env(|name| (name == "HOME").then(|| "/home/me".to_owned())).unwrap();
        assert_eq!(
            config.listen,
            SocketAddr::from((Ipv4Addr::LOCALHOST, 47616))
        );
        assert_ne!(config.listen.port(), mcp::PORT);
        assert_eq!(
            config.data_dir,
            PathBuf::from("/home/me/.local/share/toodo-server")
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_enter_the_data_directory() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let temp = tempfile::tempdir().unwrap();
        let created = temp.path().join("share/toodo-server");
        create_private_dir(&created).unwrap();
        assert_eq!(mode(&created), 0o700);

        let existing = temp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o775)).unwrap();
        create_private_dir(&existing).unwrap();
        assert_eq!(mode(&existing), 0o700);
    }
}
