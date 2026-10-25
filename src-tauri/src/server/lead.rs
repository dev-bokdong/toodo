//! The lead (see [`crate::commands::lead`]). The server keeps the lead's workspace, a
//! local git repository without a remote that is registered as the Toodo workspace
//! "Lead", and one agent running in a pane of its herdr workspace.
//!
//! Every few seconds it checks that pane. A shell waiting for input in the foreground
//! means the agent is gone, so the server types the launcher into the shell again, waiting
//! longer after each agent that exits right after it starts. After a server restart an
//! agent still running in the pane is adopted rather than started twice. An agent that
//! starts working, after it was not at the last check, has picked up the chat messages
//! typed into its pane by then, so they are marked as seen.
//!
//! While the agent runs, the server types the IDs of what waits in the lead's inbox (see
//! [`crate::domain::inbox`]) into its pane: everything pending once for each agent
//! session, then each person's message not delivered yet (stored while the lead was not
//! running, or while typing failed) and each new task event. The agent takes the items
//! themselves with the MCP tool `take_pending`.
//!
//! The launcher (`lead-start` in the data directory, readable by the owner only) exports
//! the server's MCP URL and token, the model, and the system prompt file, then runs the
//! configured command in the workspace. The workspace gets MCP configurations for omo and
//! Claude Code that read the token from the environment, so no file in it holds the token.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{fs, io, thread};

use serde_json::{Value, json};

use super::Services;
use crate::adapters::git::{Git, shell_quote};
use crate::adapters::herdr::{Herdr, PaneAgent};
use crate::commands::events::Notifier;
use crate::commands::lead::{LeadState, LeadStatus, LeadStatusState};
use crate::commands::types::{AgentStatus, Timestamp, WorkspaceSetup, WorkspaceSource};
use crate::commands::{StorageState, lock, workspaces};
use crate::domain::chat::ChatAuthor;
use crate::domain::inbox::ItemId;
use crate::domain::model::{self, HostPath, Machine, WorkspaceLocation};
use crate::domain::path::PosixPath;

/// The server-sent event that carries a changed [`LeadStatus`].
pub const LEAD_CHANGED: &str = "lead-changed";

/// The agent command unless `TOODO_LEAD_COMMAND` sets one: omo with the project's MCP
/// configuration trusted, the model if one is set, and the lead's system prompt.
pub const DEFAULT_COMMAND: &str = "omo --approve ${TOODO_LEAD_MODEL:+--model \"$TOODO_LEAD_MODEL\"} \
     --append-system-prompt \"$TOODO_LEAD_PROMPT\"";

const WORKSPACE_NAME: &str = "Lead";
const BASE_BRANCH: &str = "main";
const LAUNCHER_FILE: &str = "lead-start";
const PROMPT_FILE: &str = "lead-prompt.md";
const PROMPT: &str = include_str!("lead-prompt.md");
/// The name of this server in the lead's MCP configurations.
const MCP_SERVER: &str = "toodo";
const TOKEN_VARIABLE: &str = "TOODO_MCP_TOKEN";

const CHECK_PERIOD: Duration = Duration::from_secs(5);
/// How long a stopping server waits for a check in progress to finish.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// Seconds a launched agent has to show up in the pane's foreground.
const START_GRACE: i64 = 20;
/// Seconds after which a running agent no longer counts as a failed start.
const STABLE: i64 = 60;
/// Seconds after the lead saw a message that it stops counting as composing a reply.
const TYPING_CAP: i64 = 120;
/// Failed starts in a row after which the lead is reported as failed.
const FAILED_AFTER: u32 = 3;

/// The lead's configuration, from `TOODO_LEAD*` environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeadConfig {
    /// The lead's workspace.
    pub dir: PathBuf,
    /// Shell command that starts the agent, run in `dir`.
    pub command: String,
    pub model: Option<String>,
}

impl LeadConfig {
    /// `None` when `TOODO_LEAD` is `off`.
    pub fn from_env(
        var: impl Fn(&str) -> Option<String>,
        data_dir: &Path,
    ) -> Result<Option<Self>, String> {
        match var("TOODO_LEAD").as_deref().map(str::trim) {
            None | Some("" | "on") => {}
            Some("off") => return Ok(None),
            Some(other) => return Err(format!("TOODO_LEAD {other:?} is neither on nor off")),
        }
        let set = |name: &str| {
            var(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        Ok(Some(Self {
            dir: set("TOODO_LEAD_DIR").map_or_else(|| data_dir.join("lead"), PathBuf::from),
            command: set("TOODO_LEAD_COMMAND").unwrap_or_else(|| DEFAULT_COMMAND.to_owned()),
            model: set("TOODO_LEAD_MODEL"),
        }))
    }
}

/// What a check found the agent doing, or what it does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Running,
    /// Launched, and not in the foreground yet.
    Starting,
    /// Gone, and due to be launched.
    Launch,
    /// Gone, and waiting for its backoff to pass.
    Wait,
}

/// What the supervisor remembers about the agent between checks. Times are seconds since
/// the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Agent {
    launched_at: Option<i64>,
    running_since: Option<i64>,
    /// Agents in a row that exited before they ran for [`STABLE`] seconds.
    failures: u32,
    next_launch_at: i64,
    restarts: u32,
    /// An agent ran or was launched since the server started.
    started: bool,
}

impl Agent {
    fn observe(&mut self, busy: bool, now: i64) -> Step {
        if busy {
            let since = *self.running_since.get_or_insert(now);
            self.started = true;
            if now - since >= STABLE {
                self.failures = 0;
            }
            return Step::Running;
        }
        if let Some(at) = self.launched_at
            && self.running_since.is_none()
            && now - at < START_GRACE
        {
            return Step::Starting;
        }
        self.gone(now);
        if now >= self.next_launch_at {
            Step::Launch
        } else {
            Step::Wait
        }
    }

    /// The agent is gone: it exited, never came up, or its pane closed.
    fn gone(&mut self, now: i64) {
        if self.launched_at.is_none() && self.running_since.is_none() {
            return;
        }
        let ran = self
            .running_since
            .is_some_and(|since| now - since >= STABLE);
        self.failures = if ran { 0 } else { self.failures + 1 };
        self.next_launch_at = now + backoff(self.failures);
        self.launched_at = None;
        self.running_since = None;
    }

    fn launched(&mut self, now: i64) {
        if self.started {
            self.restarts += 1;
        }
        self.started = true;
        self.launched_at = Some(now);
    }
}

/// Seconds to wait before launching again after `failures` failed starts in a row.
fn backoff(failures: u32) -> i64 {
    match failures {
        0 => 0,
        1 => 10,
        2 => 30,
        3 => 60,
        4 => 120,
        _ => 300,
    }
}

pub struct Supervisor {
    config: LeadConfig,
    data_dir: PathBuf,
    mcp_url: String,
    mcp_token: String,
    workspace: Option<model::WorkspaceId>,
    herdr_workspace: Option<String>,
    pane: Option<String>,
    agent: Agent,
    /// The last check found the agent working.
    working: bool,
    /// The agent session (when it started) the inbox was last signalled to.
    signalled_session: Option<i64>,
    /// The newest task event signalled to that session.
    signalled_event: i64,
}

impl Supervisor {
    /// `data_dir` holds the launcher and the prompt; it should be absolute, since the
    /// launcher runs from the pane's shell.
    pub fn new(config: LeadConfig, data_dir: PathBuf, mcp_url: String, mcp_token: String) -> Self {
        Self {
            config,
            data_dir,
            mcp_url,
            mcp_token,
            workspace: None,
            herdr_workspace: None,
            pane: None,
            agent: Agent::default(),
            working: false,
            signalled_session: None,
            signalled_event: 0,
        }
    }

    /// Types the IDs of the items waiting for the lead into its pane while it runs:
    /// everything pending once per agent session, so a new session learns of what an
    /// earlier one never took, then each chat message not delivered yet and each new
    /// event. Returns whether chat messages were delivered.
    fn signal_inbox(&mut self, storage: &StorageState, herdr: &Herdr, status: &LeadStatus) -> bool {
        let (LeadState::Running, Some(pane)) = (status.state, status.pane_id.as_deref()) else {
            return false;
        };
        let session = status.started_at.map(|at| at.0);
        let new_session = session != self.signalled_session;
        let since = if new_session { 0 } else { self.signalled_event };
        let items = {
            let storage = lock(storage);
            match (storage.pending_items(), storage.undelivered_chat()) {
                (Ok(pending), Ok(undelivered)) => pending
                    .into_iter()
                    .filter(|item| match item {
                        ItemId::Chat(id) => new_session || undelivered.contains(id),
                        ItemId::Event(id) => *id > since,
                    })
                    .collect::<Vec<_>>(),
                (Err(error), _) | (_, Err(error)) => {
                    eprintln!("toodo-server: cannot read the lead's inbox: {error}");
                    return false;
                }
            }
        };
        if !items.is_empty()
            && let Err(error) = crate::commands::lead::signal(storage, herdr, pane, &items)
        {
            eprintln!("toodo-server: cannot signal the lead's inbox: {error}");
            return false;
        }
        self.signalled_session = session;
        self.signalled_event = items
            .iter()
            .filter_map(|item| match item {
                ItemId::Event(id) => Some(*id),
                ItemId::Chat(_) => None,
            })
            .fold(since, i64::max);
        items.iter().any(|item| matches!(item, ItemId::Chat(_)))
    }

    /// Marks the chat messages delivered by `now`, when the check that started then found
    /// the agent working while the one before did not, as seen: the agent started a turn
    /// and took them in. Returns whether any were marked.
    fn note_turn(&mut self, storage: &StorageState, status: &LeadStatus, now: i64) -> bool {
        let working =
            status.state == LeadState::Running && status.agent_status == Some(AgentStatus::Working);
        let started = working && !self.working;
        self.working = working;
        if !started {
            return false;
        }
        match lock(storage).mark_chat_seen(now) {
            Ok(marked) => marked > 0,
            Err(error) => {
                eprintln!("toodo-server: cannot mark chat messages as seen: {error}");
                false
            }
        }
    }

    /// Whether the lead is composing a reply at `now`: it is working, the newest chat
    /// message is a person's that it saw less than [`TYPING_CAP`] seconds ago. A stored
    /// reply, an idle agent, or the cap ends it.
    fn typing(storage: &StorageState, status: &LeadStatus, now: i64) -> bool {
        let working =
            status.state == LeadState::Running && status.agent_status == Some(AgentStatus::Working);
        if !working {
            return false;
        }
        match lock(storage).latest_chat_message() {
            Ok(Some(message)) => {
                message.author == ChatAuthor::User
                    && message
                        .seen_at
                        .is_some_and(|seen| (0..TYPING_CAP).contains(&(now - seen)))
            }
            Ok(None) => false,
            Err(error) => {
                eprintln!("toodo-server: cannot read the newest chat message: {error}");
                false
            }
        }
    }

    /// Checks the lead once, starting its agent when it is due.
    fn check(&mut self, storage: &StorageState, git: &Git, herdr: &Herdr, now: i64) -> LeadStatus {
        match self.try_check(storage, git, herdr, now) {
            Ok(status) => status,
            Err(error) => LeadStatus {
                state: LeadState::Failed,
                error: Some(error),
                ..self.status(None, Step::Wait)
            },
        }
    }

    fn try_check(
        &mut self,
        storage: &StorageState,
        git: &Git,
        herdr: &Herdr,
        now: i64,
    ) -> Result<LeadStatus, String> {
        let dir = workspace_dir(&self.config.dir)?;
        self.workspace = Some(ensure_workspace(storage, git, herdr, &dir)?.id);
        let pane = self.find_pane(herdr, &dir, now)?;
        let process = herdr
            .pane_process(&pane.pane_id)
            .map_err(|error| error.to_string())?;
        let step = self.agent.observe(process.busy(), now);
        if step == Step::Launch {
            self.launch(herdr, &dir, &pane.pane_id)?;
            self.agent.launched(now);
        }
        Ok(self.status(Some(&pane), step))
    }

    /// The tracked pane of the lead's herdr workspace, or else the one an agent runs in, or
    /// else the first. The workspace is the tracked one, or else the one with a pane in the
    /// lead's directory (herdr does not report the checkout of a workspace it opened on a
    /// plain repository), or else it is opened anew.
    fn find_pane(&mut self, herdr: &Herdr, dir: &str, now: i64) -> Result<PaneAgent, String> {
        let local = Machine::Local;
        let mut panes = herdr
            .list_panes(&local)
            .map_err(|error| error.to_string())?;
        let open = self
            .herdr_workspace
            .clone()
            .filter(|id| panes.iter().any(|pane| &pane.workspace_id == id))
            .or_else(|| {
                panes
                    .iter()
                    .filter(|pane| pane.cwd.as_deref() == Some(dir))
                    .max_by_key(|pane| pane.agent.is_some())
                    .map(|pane| pane.workspace_id.clone())
            });
        let workspace = match open {
            Some(id) => id,
            None => {
                let id = herdr
                    .open_workspace(&local, dir, WORKSPACE_NAME)
                    .map_err(|error| error.to_string())?;
                panes = herdr
                    .list_panes(&local)
                    .map_err(|error| error.to_string())?;
                id
            }
        };
        panes.retain(|pane| pane.workspace_id == workspace);
        self.herdr_workspace = Some(workspace.clone());
        let tracked = self
            .pane
            .as_ref()
            .and_then(|id| panes.iter().find(|pane| &pane.pane_id == id));
        let pane = match tracked {
            Some(pane) => pane.clone(),
            None => {
                if self.pane.is_some() {
                    self.agent.gone(now);
                }
                panes
                    .iter()
                    .find(|pane| pane.agent.is_some())
                    .or_else(|| panes.first())
                    .cloned()
                    .ok_or_else(|| format!("herdr workspace {workspace} has no pane"))?
            }
        };
        self.pane = Some(pane.pane_id.clone());
        Ok(pane)
    }

    /// Writes the prompt, the launcher, and the MCP configurations, then runs the launcher
    /// in `pane`.
    fn launch(&self, herdr: &Herdr, dir: &str, pane: &str) -> Result<(), String> {
        let prompt = self.data_dir.join(PROMPT_FILE);
        let launcher = self.data_dir.join(LAUNCHER_FILE);
        let failed = |path: &Path, error: io::Error| format!("{}: {error}", path.display());
        fs::write(&prompt, PROMPT).map_err(|error| failed(&prompt, error))?;
        let script = launcher_script(&Launch {
            dir,
            command: &self.config.command,
            model: self.config.model.as_deref(),
            prompt: &path_text(&prompt)?,
            mcp_url: &self.mcp_url,
            mcp_token: &self.mcp_token,
        });
        write_private(&launcher, &script).map_err(|error| failed(&launcher, error))?;
        for (file, config) in [
            (Path::new(dir).join(".omo/mcp.json"), omo_mcp(&self.mcp_url)),
            (Path::new(dir).join(".mcp.json"), claude_mcp(&self.mcp_url)),
        ] {
            merge_mcp_config(&file, config).map_err(|error| failed(&file, error))?;
        }
        let command = format!("sh {}", shell_quote(&path_text(&launcher)?));
        herdr
            .run_in_pane(pane, &command)
            .map_err(|error| error.to_string())
    }

    fn status(&self, pane: Option<&PaneAgent>, step: Step) -> LeadStatus {
        let agent = &self.agent;
        let (state, error) = match step {
            Step::Running => (LeadState::Running, None),
            Step::Starting | Step::Launch => (LeadState::Starting, None),
            Step::Wait if agent.failures >= FAILED_AFTER => (
                LeadState::Failed,
                Some(format!(
                    "the agent exited right after it started {} times in a row; check \
                     TOODO_LEAD_COMMAND and the lead's pane",
                    agent.failures
                )),
            ),
            // Stable while it waits, so that the status (and the log) changes only when
            // something happens rather than with every check.
            Step::Wait => (
                LeadState::Restarting,
                Some(format!(
                    "the agent exited right after it started; it starts again {} s after \
                     it exited",
                    backoff(agent.failures)
                )),
            ),
        };
        LeadStatus {
            state,
            workspace_id: self.workspace.map(Into::into),
            herdr_workspace_id: self.herdr_workspace.clone(),
            pane_id: self.pane.clone(),
            agent: pane.and_then(|pane| pane.agent.clone()),
            agent_status: pane.and_then(|pane| AgentStatus::from_herdr(pane.status)),
            typing: false,
            restarts: agent.restarts,
            started_at: agent.running_since.or(agent.launched_at).map(Timestamp),
            error,
        }
    }
}

/// Checks the lead every few seconds on a thread of its own, publishing its status in
/// `services` and as a [`LEAD_CHANGED`] event whenever it changes.
pub fn spawn(services: Arc<Services>, mut supervisor: Supervisor) -> io::Result<Checks> {
    every(CHECK_PERIOD, move || {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                elapsed.as_secs().try_into().unwrap_or(i64::MAX)
            });
        let mut status = supervisor.check(&services.storage, &services.git, &services.herdr, now);
        let delivered = supervisor.signal_inbox(&services.storage, &services.herdr, &status);
        if supervisor.note_turn(&services.storage, &status, now) || delivered {
            services.chat_changed();
        }
        status.typing = Supervisor::typing(&services.storage, &status, now);
        if let Some(previous) = publish(&services.lead, &status) {
            if (previous.state, &previous.error) != (status.state, &status.error) {
                let reason = status.error.as_deref().unwrap_or("");
                println!("toodo-server: lead {:?} {reason}", status.state);
            }
            services.emit(LEAD_CHANGED, &status);
        }
    })
}

/// The thread that checks the lead, until it is stopped.
pub struct Checks {
    stopped: Arc<AtomicBool>,
    thread: thread::Thread,
    /// Disconnects when the thread ends.
    ended: mpsc::Receiver<()>,
}

impl Checks {
    /// Stops the checks, waiting up to `timeout` for one in progress to finish so that the
    /// published status names every herdr workspace a check opened. Returns whether the
    /// thread ended in time.
    pub fn stop(&self, timeout: Duration) -> bool {
        self.stopped.store(true, Ordering::SeqCst);
        self.thread.unpark();
        matches!(
            self.ended.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        )
    }
}

/// Runs `check` on a thread of its own, then again `period` after each run, until stopped.
fn every(period: Duration, mut check: impl FnMut() + Send + 'static) -> io::Result<Checks> {
    let stopped = Arc::new(AtomicBool::new(false));
    let (end, ended) = mpsc::channel::<()>();
    let stop = stopped.clone();
    let handle = thread::Builder::new()
        .name("lead".to_owned())
        .spawn(move || {
            let _end = end;
            while !stop.load(Ordering::SeqCst) {
                check();
                let next = Instant::now() + period;
                while !stop.load(Ordering::SeqCst) {
                    let left = next.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    thread::park_timeout(left);
                }
            }
        })?;
    Ok(Checks {
        stopped,
        thread: handle.thread().clone(),
        ended,
    })
}

/// Closes the lead's herdr workspace (and with it the agent) when the server stops, so a
/// stopped server leaves nothing running in herdr; the next start opens it anew.
pub fn close(lead: &LeadStatusState, herdr: &Herdr) {
    let id = lead
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .herdr_workspace_id
        .clone();
    let Some(id) = id else { return };
    if let Err(error) = herdr.close_workspace(&Machine::Local, &id) {
        eprintln!("toodo-server: cannot close the lead's herdr workspace {id}: {error}");
    }
}

/// Stores `status`, returning the one it replaced if it differs.
fn publish(lead: &LeadStatusState, status: &LeadStatus) -> Option<LeadStatus> {
    let mut current = lead.lock().unwrap_or_else(PoisonError::into_inner);
    (*current != *status).then(|| std::mem::replace(&mut *current, status.clone()))
}

/// Creates the lead's workspace directory and returns its absolute path.
fn workspace_dir(dir: &Path) -> Result<String, String> {
    fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let dir = fs::canonicalize(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    path_text(&dir)
}

fn path_text(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))
}

/// The Toodo workspace at `dir`, created as a new repository without a remote (and opened
/// in herdr) when there is none.
fn ensure_workspace(
    storage: &StorageState,
    git: &Git,
    herdr: &Herdr,
    dir: &str,
) -> Result<model::Workspace, String> {
    let path = PosixPath::parse(dir).map_err(|error| error.to_string())?;
    let location = WorkspaceLocation::Local {
        path: HostPath::Posix(path),
    };
    let existing = lock(storage)
        .list_workspaces()
        .map_err(|error| error.to_string())?;
    if let Some(workspace) = existing.into_iter().find(|w| w.location == location) {
        return Ok(workspace);
    }
    let setup = WorkspaceSetup {
        name: WORKSPACE_NAME.to_owned(),
        location,
        source: WorkspaceSource::Init,
        default_base_branch: Some(BASE_BRANCH.to_owned()),
    };
    workspaces::create(storage, git, herdr, setup).map_err(|error| error.message)
}

struct Launch<'a> {
    dir: &'a str,
    command: &'a str,
    model: Option<&'a str>,
    prompt: &'a str,
    mcp_url: &'a str,
    mcp_token: &'a str,
}

fn launcher_script(launch: &Launch<'_>) -> String {
    let variables = [
        ("TOODO_LEAD_DIR", launch.dir),
        ("TOODO_LEAD_MODEL", launch.model.unwrap_or("")),
        ("TOODO_LEAD_PROMPT", launch.prompt),
        ("TOODO_MCP_URL", launch.mcp_url),
        (TOKEN_VARIABLE, launch.mcp_token),
    ];
    let mut script = String::from(
        "#!/bin/sh\n# Written by toodo-server each time it starts the lead; set \
         TOODO_LEAD_COMMAND and TOODO_LEAD_MODEL instead of editing it.\n",
    );
    for (name, value) in variables {
        script.push_str(&format!("{name}={}\nexport {name}\n", shell_quote(value)));
    }
    script.push_str(&format!(
        "cd {} || exit 1\n{}\n",
        shell_quote(launch.dir),
        launch.command
    ));
    script
}

/// omo's MCP server entry: it sends the token from the environment.
fn omo_mcp(url: &str) -> Value {
    json!({ "url": url, "bearerTokenEnv": TOKEN_VARIABLE })
}

/// Claude Code's MCP server entry: it expands the variable in the header.
fn claude_mcp(url: &str) -> Value {
    json!({
        "type": "http",
        "url": url,
        "headers": { "Authorization": format!("Bearer ${{{TOKEN_VARIABLE}}}") },
    })
}

/// Sets this server's entry in the MCP configuration at `file`, keeping other servers. A
/// file that is not a JSON object with an object `mcpServers` (say, one with comments) is
/// left alone and reported, rather than replaced along with the servers it lists.
fn merge_mcp_config(file: &Path, server: Value) -> io::Result<()> {
    let unreadable = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a JSON object with an object mcpServers; fix or remove it",
        )
    };
    let mut config = match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(unreadable)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error),
    };
    match config.get("mcpServers") {
        None => config["mcpServers"] = json!({}),
        Some(servers) if servers.is_object() => {}
        Some(_) => return Err(unreadable()),
    }
    config["mcpServers"][MCP_SERVER] = server;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&config).map_err(io::Error::other)?;
    fs::write(file, text + "\n")
}

/// Writes `contents` to `path` so that on Unix only the owner can read it, the file
/// holding the MCP token.
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let file = options.open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        io::Write::write_all(&mut &file, contents.as_bytes())
    }
    #[cfg(not(unix))]
    io::Write::write_all(&mut options.open(path)?, contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::adapters::git::fake::FakeRunner;
    use crate::storage::Storage;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn the_lead_runs_omo_in_the_data_directory_unless_configured_otherwise() {
        let data = Path::new("/srv/toodo");
        let default = LeadConfig::from_env(env(&[]), data).unwrap().unwrap();
        assert_eq!(default.dir, PathBuf::from("/srv/toodo/lead"));
        assert_eq!(default.command, DEFAULT_COMMAND);
        assert_eq!(default.model, None);
        let configured = LeadConfig::from_env(
            env(&[
                ("TOODO_LEAD", "on"),
                ("TOODO_LEAD_DIR", "/home/me/lead"),
                ("TOODO_LEAD_COMMAND", "claude --model \"$TOODO_LEAD_MODEL\""),
                ("TOODO_LEAD_MODEL", " opus "),
            ]),
            data,
        )
        .unwrap()
        .unwrap();
        assert_eq!(configured.dir, PathBuf::from("/home/me/lead"));
        assert_eq!(configured.command, "claude --model \"$TOODO_LEAD_MODEL\"");
        assert_eq!(configured.model.as_deref(), Some("opus"));
        assert_eq!(
            LeadConfig::from_env(env(&[("TOODO_LEAD", "off")]), data),
            Ok(None)
        );
        assert!(LeadConfig::from_env(env(&[("TOODO_LEAD", "maybe")]), data).is_err());
    }

    #[test]
    fn a_missing_agent_is_launched_and_given_time_to_come_up() {
        let mut agent = Agent::default();
        assert_eq!(agent.observe(false, 100), Step::Launch);
        agent.launched(100);
        assert_eq!(agent.observe(false, 105), Step::Starting);
        assert_eq!(agent.observe(true, 110), Step::Running);
        assert_eq!(agent.restarts, 0);
        assert_eq!(agent.running_since, Some(110));
    }

    #[test]
    fn an_agent_that_ran_for_a_while_is_restarted_at_once() {
        let mut agent = Agent::default();
        agent.observe(false, 0);
        agent.launched(0);
        agent.observe(true, 5);
        assert_eq!(agent.observe(true, 5 + STABLE), Step::Running);
        assert_eq!(agent.observe(false, 200), Step::Launch);
        agent.launched(200);
        assert_eq!((agent.restarts, agent.failures), (1, 0));
    }

    #[test]
    fn agents_that_exit_right_away_are_restarted_later_each_time() {
        let mut agent = Agent::default();
        agent.observe(false, 0);
        agent.launched(0);
        // The command never came up.
        assert_eq!(agent.observe(false, START_GRACE), Step::Wait);
        assert_eq!(agent.failures, 1);
        assert_eq!(agent.observe(false, START_GRACE + 9), Step::Wait);
        assert_eq!(agent.observe(false, START_GRACE + 10), Step::Launch);
        agent.launched(START_GRACE + 10);
        // It came up but exited within a minute.
        agent.observe(true, 40);
        assert_eq!(agent.observe(false, 50), Step::Wait);
        assert_eq!((agent.failures, agent.next_launch_at), (2, 80));
        assert_eq!(agent.observe(false, 80), Step::Launch);
        assert_eq!(agent.restarts, 1);
    }

    #[test]
    fn an_agent_found_running_is_adopted_rather_than_launched() {
        let mut agent = Agent::default();
        assert_eq!(agent.observe(true, 0), Step::Running);
        assert_eq!(agent.launched_at, None);
        assert_eq!(agent.observe(false, 10), Step::Wait);
        assert_eq!(agent.observe(false, 20), Step::Launch);
        agent.launched(20);
        assert_eq!(agent.restarts, 1);
    }

    #[test]
    fn the_launcher_exports_the_token_and_runs_the_command_in_the_workspace() {
        let script = launcher_script(&Launch {
            dir: "/srv/it's/lead",
            command: "omo --approve",
            model: Some("opus"),
            prompt: "/srv/lead-prompt.md",
            mcp_url: "http://127.0.0.1:47616/mcp",
            mcp_token: "secret",
        });
        assert!(script.starts_with("#!/bin/sh\n"), "{script}");
        for line in [
            "TOODO_MCP_TOKEN='secret'\nexport TOODO_MCP_TOKEN\n",
            "TOODO_MCP_URL='http://127.0.0.1:47616/mcp'\n",
            "TOODO_LEAD_MODEL='opus'\n",
            "TOODO_LEAD_PROMPT='/srv/lead-prompt.md'\n",
            "cd '/srv/it'\\''s/lead' || exit 1\nomo --approve\n",
        ] {
            assert!(script.contains(line), "{line:?} missing from {script}");
        }
        assert!(script.ends_with("omo --approve\n"), "{script}");
    }

    #[test]
    fn mcp_configurations_name_this_server_without_its_token_and_keep_others() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".omo/mcp.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            r#"{"settings":{"x":1},"mcpServers":{"docs":{"command":"docs"},"toodo":{"url":"old"}}}"#,
        )
        .unwrap();
        merge_mcp_config(&file, omo_mcp("http://127.0.0.1:1/mcp")).unwrap();
        let claude = dir.path().join(".mcp.json");
        merge_mcp_config(&claude, claude_mcp("http://127.0.0.1:1/mcp")).unwrap();
        let read = |path: &Path| -> Value {
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
        };
        assert_eq!(
            read(&file),
            json!({
                "settings": { "x": 1 },
                "mcpServers": {
                    "docs": { "command": "docs" },
                    "toodo": { "url": "http://127.0.0.1:1/mcp", "bearerTokenEnv": "TOODO_MCP_TOKEN" },
                },
            })
        );
        assert_eq!(
            read(&claude)["mcpServers"]["toodo"]["headers"]["Authorization"],
            "Bearer ${TOODO_MCP_TOKEN}"
        );
    }

    #[test]
    fn an_mcp_configuration_that_is_not_plain_json_is_reported_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("comments.json", "// mine\n{\"mcpServers\":{\"docs\":{}}}\n"),
            ("servers.json", "{\"mcpServers\":[\"docs\"]}"),
        ] {
            let file = dir.path().join(name);
            fs::write(&file, text).unwrap();
            let error = merge_mcp_config(&file, omo_mcp("http://127.0.0.1:1/mcp")).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text, "{name}");
        }
    }

    #[test]
    fn stopping_the_checks_waits_for_the_one_in_progress_and_runs_no_more() {
        let (entered, check_entered) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let runs = Arc::new(Mutex::new(0));
        let counted = runs.clone();
        let checks = every(Duration::ZERO, move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            *counted.lock().unwrap() += 1;
        })
        .unwrap();
        check_entered.recv().unwrap();
        // The check cannot finish before it is released.
        assert!(!checks.stop(Duration::from_millis(50)));
        release.send(()).unwrap();
        assert!(checks.stop(Duration::from_secs(60)));
        assert_eq!(*runs.lock().unwrap(), 1);
    }

    fn herdr_output(json: Value) -> Result<String, crate::adapters::git::GitError> {
        Ok(json.to_string())
    }

    fn pane_list(panes: &[(&str, &str, &str, Option<&str>)]) -> Value {
        let rows: Vec<Value> = panes
            .iter()
            .map(|(pane, workspace, cwd, agent)| {
                let mut row = json!({
                    "pane_id": pane,
                    "workspace_id": workspace,
                    "cwd": cwd,
                    "agent_status": if agent.is_some() { "idle" } else { "unknown" },
                });
                if let Some(agent) = agent {
                    row["agent"] = json!(agent);
                }
                row
            })
            .collect();
        json!({ "id": "x", "result": { "panes": rows } })
    }

    fn process(shell: u32, foreground: u32) -> Value {
        json!({ "id": "x", "result": { "process_info": {
            "shell_pid": shell, "foreground_process_group_id": foreground,
        } } })
    }

    #[test]
    fn the_first_check_creates_the_workspace_and_launches_the_agent_in_its_pane() {
        let temp = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(temp.path()).unwrap();
        let dir = path_text(&data.join("lead")).unwrap();
        let git_runner = Arc::new(FakeRunner::new([Ok(String::new()), Ok(format!("{dir}\n"))]));
        let git = Git::new(Box::new(git_runner.clone()));
        let herdr_runner = Arc::new(FakeRunner::new([
            // Creating the Toodo workspace opens it in herdr.
            herdr_output(json!({ "id": "x", "result": { "workspaces": [] } })),
            herdr_output(json!({ "id": "x", "result": { "workspace": { "workspace_id": "w7" } } })),
            // The supervisor finds that workspace by its pane in the lead's directory.
            herdr_output(pane_list(&[
                ("other:p1", "other", "/src", Some("omo")),
                ("w7:p1", "w7", dir.as_str(), None),
            ])),
            herdr_output(process(10, 10)),
            Ok(String::new()),
            // Second check: the agent runs.
            herdr_output(pane_list(&[("w7:p1", "w7", dir.as_str(), Some("omo"))])),
            herdr_output(process(10, 20)),
        ]));
        let herdr = Herdr::new(Box::new(herdr_runner.clone()));
        let storage: StorageState = Mutex::new(Storage::open_in_memory().unwrap());
        let config = LeadConfig::from_env(env(&[("TOODO_LEAD_MODEL", "opus")]), &data)
            .unwrap()
            .unwrap();
        let mut supervisor = Supervisor::new(
            config,
            data.clone(),
            "http://127.0.0.1:47616/mcp".to_owned(),
            "secret".to_owned(),
        );

        let first = supervisor.check(&storage, &git, &herdr, 1_000);
        assert_eq!(first.state, LeadState::Starting, "{first:?}");
        assert_eq!(first.pane_id.as_deref(), Some("w7:p1"));
        assert_eq!(first.herdr_workspace_id.as_deref(), Some("w7"));
        assert_eq!(first.started_at, Some(Timestamp(1_000)));
        let workspaces = lock(&storage).list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "Lead");
        assert_eq!(first.workspace_id, Some(workspaces[0].id.into()));
        let launcher = data.join(LAUNCHER_FILE);
        let run = herdr_runner.invocations.lock().unwrap()[4].args.clone();
        assert_eq!(
            run,
            [
                "pane",
                "run",
                "w7:p1",
                &format!("sh '{}'", launcher.display())
            ]
        );
        let script = fs::read_to_string(&launcher).unwrap();
        assert!(script.contains("TOODO_MCP_TOKEN='secret'"), "{script}");
        assert!(script.contains(&format!("cd '{dir}'")), "{script}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&launcher).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(fs::read_to_string(data.join(PROMPT_FILE)).unwrap(), PROMPT);
        let omo = fs::read_to_string(Path::new(&dir).join(".omo/mcp.json")).unwrap();
        assert!(omo.contains("47616") && !omo.contains("secret"), "{omo}");

        let second = supervisor.check(&storage, &git, &herdr, 1_010);
        assert_eq!(second.state, LeadState::Running, "{second:?}");
        assert_eq!(second.agent.as_deref(), Some("omo"));
        assert_eq!(second.agent_status, Some(AgentStatus::Idle));
        assert_eq!(
            (second.restarts, second.started_at),
            (0, Some(Timestamp(1_010)))
        );
        assert!(herdr_runner.results.lock().unwrap().is_empty());
        assert_eq!(git_runner.invocations.lock().unwrap().len(), 2);
    }

    #[test]
    fn messages_delivered_before_the_agent_starts_working_are_seen() {
        use crate::domain::chat::ChatAuthor;

        let storage: StorageState = Mutex::new(Storage::open_in_memory().unwrap());
        let add = |at: i64, client: &str| {
            let mut storage = lock(&storage);
            storage.set_clock(move || at);
            let message = storage
                .add_chat_message(ChatAuthor::User, "Hi", Some(client))
                .unwrap();
            storage.mark_chat_delivered(&[message.id]).unwrap();
        };
        let seen = || -> Vec<Option<i64>> {
            let messages = lock(&storage).list_chat_messages().unwrap();
            messages.iter().map(|message| message.seen_at).collect()
        };
        let config = LeadConfig::from_env(env(&[]), Path::new("/srv"))
            .unwrap()
            .unwrap();
        let mut supervisor = Supervisor::new(
            config,
            "/srv".into(),
            "http://x/mcp".to_owned(),
            "t".to_owned(),
        );
        let status = |agent_status| LeadStatus {
            state: LeadState::Running,
            agent_status: Some(agent_status),
            ..LeadStatus::default()
        };

        add(100, "c1");
        assert!(!supervisor.note_turn(&storage, &status(AgentStatus::Idle), 105));
        assert_eq!(seen(), [None]);
        assert!(supervisor.note_turn(&storage, &status(AgentStatus::Working), 110));
        assert_eq!(seen(), [Some(100)]);

        // Typed in while the agent was busy with a turn it had already started.
        add(112, "c2");
        assert!(!supervisor.note_turn(&storage, &status(AgentStatus::Working), 115));
        assert!(!supervisor.note_turn(&storage, &status(AgentStatus::Done), 120));
        assert_eq!(seen(), [Some(100), None]);
        // A lead that is not running is not working, whatever herdr last said.
        let restarting = LeadStatus {
            state: LeadState::Restarting,
            ..status(AgentStatus::Working)
        };
        assert!(!supervisor.note_turn(&storage, &restarting, 122));
        assert!(supervisor.note_turn(&storage, &status(AgentStatus::Working), 125));
        assert_eq!(seen(), [Some(100), Some(112)]);
    }

    #[test]
    fn the_lead_types_from_seeing_a_message_until_it_replies_goes_idle_or_hits_the_cap() {
        use crate::domain::chat::ChatAuthor;

        let storage: StorageState = Mutex::new(Storage::open_in_memory().unwrap());
        let add = |at: i64, author: ChatAuthor| {
            let mut storage = lock(&storage);
            storage.set_clock(move || at);
            let message = storage.add_chat_message(author, "Hi", None).unwrap();
            if author == ChatAuthor::User {
                storage.mark_chat_delivered(&[message.id]).unwrap();
            }
        };
        let status = |state, agent_status| LeadStatus {
            state,
            agent_status: Some(agent_status),
            ..LeadStatus::default()
        };
        let working = status(LeadState::Running, AgentStatus::Working);
        let typing = |status: &LeadStatus, now| Supervisor::typing(&storage, status, now);

        assert!(!typing(&working, 100), "no messages yet");
        add(100, ChatAuthor::User);
        assert!(!typing(&working, 101), "delivered but not seen");
        {
            let mut storage = lock(&storage);
            storage.set_clock(|| 102);
            storage.mark_chat_seen(100).unwrap();
        }
        assert!(typing(&working, 103));
        assert!(!typing(&status(LeadState::Running, AgentStatus::Idle), 103));
        assert!(!typing(
            &status(LeadState::Restarting, AgentStatus::Working),
            103
        ));
        assert!(typing(&working, 102 + TYPING_CAP - 1));
        assert!(!typing(&working, 102 + TYPING_CAP));

        add(110, ChatAuthor::Lead);
        assert!(!typing(&working, 111), "the reply ends it");
    }

    #[test]
    fn a_message_stored_but_never_typed_reaches_the_lead_once_it_runs() {
        use crate::domain::chat::ChatAuthor;

        let storage: StorageState = Mutex::new(Storage::open_in_memory().unwrap());
        // The server stopped after storing the message and before typing its ID.
        let message = lock(&storage)
            .add_chat_message(ChatAuthor::User, "Plan it", Some("c1"))
            .unwrap();
        let runner = Arc::new(FakeRunner::new([
            Ok(String::new()),
            Ok(String::new()),
            Ok(String::new()),
        ]));
        let herdr = Herdr::new(Box::new(runner.clone()));
        let config = LeadConfig::from_env(env(&[]), Path::new("/srv"))
            .unwrap()
            .unwrap();
        let mut supervisor = Supervisor::new(
            config,
            "/srv".into(),
            "http://x/mcp".to_owned(),
            "t".to_owned(),
        );
        let session = |started| LeadStatus {
            state: LeadState::Running,
            pane_id: Some("wL:p1".to_owned()),
            started_at: Some(Timestamp(started)),
            ..LeadStatus::default()
        };
        let typed = || -> Vec<String> {
            runner
                .invocations
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.args[3].clone())
                .collect()
        };
        let line = |ids: &str| {
            format!("[Toodo inbox] {ids} - take them with the Toodo MCP tool take_pending")
        };

        let restarting = LeadStatus {
            state: LeadState::Restarting,
            ..session(1)
        };
        assert!(!supervisor.signal_inbox(&storage, &herdr, &restarting));
        assert!(typed().is_empty());

        assert!(supervisor.signal_inbox(&storage, &herdr, &session(1)));
        assert_eq!(typed(), [line(&format!("chat-{}", message.id))]);
        assert!(lock(&storage).undelivered_chat().unwrap().is_empty());
        assert!(!supervisor.signal_inbox(&storage, &herdr, &session(1)));
        assert_eq!(typed().len(), 1, "signalled once per session");

        // A task the lead follows awaits completion: only the new event is signalled.
        let event = {
            let mut storage = lock(&storage);
            let workspace = storage
                .create_workspace(&crate::storage::NewWorkspace {
                    name: "w".to_owned(),
                    location: WorkspaceLocation::Local {
                        path: HostPath::Posix(PosixPath::parse("/w").unwrap()),
                    },
                    default_base_branch: "main".to_owned(),
                })
                .unwrap();
            let task = storage
                .create_task(&crate::storage::NewTask {
                    workspace_id: workspace.id,
                    task_type: model::TaskType::Docs,
                    title: "t".to_owned(),
                    description: String::new(),
                    estimate: None,
                    due_date: None,
                    criteria: Vec::new(),
                })
                .unwrap();
            storage.record_event_for_tests(task.id, crate::domain::status::TaskStatus::Done, 5);
            storage.pending_items().unwrap()[1]
        };
        assert!(!supervisor.signal_inbox(&storage, &herdr, &session(1)));
        assert_eq!(typed()[1], line(&event.to_string()));

        // A new session, after the agent restarted, hears of everything it has not taken.
        assert!(supervisor.signal_inbox(&storage, &herdr, &session(9)));
        assert_eq!(typed()[2], line(&format!("chat-{} {event}", message.id)));
        assert!(runner.results.lock().unwrap().is_empty());
    }

    #[test]
    fn closing_the_lead_closes_its_herdr_workspace_only_when_it_has_one() {
        let runner = Arc::new(FakeRunner::new([Ok(String::new())]));
        let herdr = Herdr::new(Box::new(runner.clone()));
        let lead = LeadStatusState::default();

        close(&lead, &herdr);
        assert!(runner.invocations.lock().unwrap().is_empty());

        lead.lock().unwrap().herdr_workspace_id = Some("w7".to_owned());
        close(&lead, &herdr);
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].args, ["workspace", "close", "w7"]);
    }

    #[test]
    fn herdr_failures_are_reported_and_retried() {
        let temp = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(temp.path()).unwrap();
        let storage: StorageState = Mutex::new(Storage::open_in_memory().unwrap());
        let dir = path_text(&data.join("lead")).unwrap();
        lock(&storage)
            .create_workspace(&crate::storage::NewWorkspace {
                name: "Lead".to_owned(),
                location: WorkspaceLocation::Local {
                    path: HostPath::Posix(PosixPath::parse(&dir).unwrap()),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap();
        let git = Git::new(Box::new(Arc::new(FakeRunner::new([]))));
        let herdr = Herdr::new(Box::new(Arc::new(FakeRunner::new([Err(
            crate::adapters::git::fake::failed(),
        )]))));
        let config = LeadConfig::from_env(env(&[]), &data).unwrap().unwrap();
        let mut supervisor =
            Supervisor::new(config, data, "http://x/mcp".to_owned(), "t".to_owned());
        let status = supervisor.check(&storage, &git, &herdr, 0);
        assert_eq!(status.state, LeadState::Failed);
        assert!(status.error.is_some_and(|error| error.contains("fatal")));
    }
}
