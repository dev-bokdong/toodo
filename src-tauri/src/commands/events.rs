//! Keeps Toodo in sync with the local herdr server through its event subscription.
//!
//! A background thread subscribes to herdr events and:
//! - forgets the worktree of a task whose worktree was removed in herdr,
//! - remembers the new herdr workspace of a task whose worktree was opened again,
//! - keeps the agent status of each task with a herdr workspace, read again from
//!   `herdr pane list` whenever panes or agents change,
//!
//! and tells the frontend through the [`TasksChanged`] and [`AgentStatusesChanged`]
//! events. When the subscription ends, e.g. herdr stopped or reported `events_lost`, it
//! subscribes again after a growing delay and reconciles with a fresh pane list. Only
//! the local herdr server is watched: tasks of SSH workspaces get no events.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use super::types::{AgentStatus, TaskAgentStatus, WorkspaceId};
use super::{HerdrState, StorageState, lock};
use crate::adapters::herdr::{self, Herdr, HerdrError, ServerStatus};
use crate::adapters::herdr_events::{
    Connector, EventStreamError, Flow, HerdrEvent, LocalSocket, Message, subscribe,
};
use crate::domain::model::{self, HostPath, WorkspaceLocation};
use crate::domain::path::WindowsPath;
use crate::storage::StorageError;

/// Agent statuses of tasks, ordered by task ID.
pub type AgentStatusState = Mutex<Vec<TaskAgentStatus>>;

/// Tasks of the workspace changed outside a command, e.g. herdr removed a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TasksChanged {
    pub workspace_id: WorkspaceId,
}

/// The agent statuses of tasks changed; carries all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
pub struct AgentStatusesChanged {
    pub statuses: Vec<TaskAgentStatus>,
}

#[tauri::command]
#[specta::specta]
pub fn list_agent_statuses(statuses: State<'_, AgentStatusState>) -> Vec<TaskAgentStatus> {
    lock_statuses(&statuses).clone()
}

fn lock_statuses(statuses: &AgentStatusState) -> MutexGuard<'_, Vec<TaskAgentStatus>> {
    statuses.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where changes are announced; the app emits them to the frontend.
pub trait Notifier {
    fn tasks_changed(&self, workspace: model::WorkspaceId);
    fn agent_statuses_changed(&self, statuses: &[TaskAgentStatus]);
}

impl Notifier for AppHandle {
    fn tasks_changed(&self, workspace: model::WorkspaceId) {
        let event = TasksChanged {
            workspace_id: workspace.into(),
        };
        if let Err(error) = event.emit(self) {
            eprintln!("toodo: failed to emit {}: {error}", TasksChanged::NAME);
        }
    }

    fn agent_statuses_changed(&self, statuses: &[TaskAgentStatus]) {
        let event = AgentStatusesChanged {
            statuses: statuses.to_vec(),
        };
        if let Err(error) = event.emit(self) {
            eprintln!(
                "toodo: failed to emit {}: {error}",
                AgentStatusesChanged::NAME
            );
        }
    }
}

#[derive(Debug)]
pub enum SyncError {
    NotRunning,
    Herdr(HerdrError),
    Stream(EventStreamError),
    Storage(StorageError),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(f, "the herdr server is not running"),
            Self::Herdr(error) => error.fmt(f),
            Self::Stream(error) => error.fmt(f),
            Self::Storage(error) => error.fmt(f),
        }
    }
}

impl From<HerdrError> for SyncError {
    fn from(error: HerdrError) -> Self {
        Self::Herdr(error)
    }
}

impl From<StorageError> for SyncError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

/// How one subscription ended.
#[derive(Debug)]
pub struct SessionEnd {
    /// Whether herdr accepted the subscription.
    pub started: bool,
    /// `None` when new panes need a new subscription.
    pub error: Option<SyncError>,
}

pub struct EventSync<'a> {
    pub storage: &'a StorageState,
    pub herdr: &'a Herdr,
    pub statuses: &'a AgentStatusState,
    pub notifier: &'a dyn Notifier,
}

impl EventSync<'_> {
    /// Subscribes once, to lifecycle events and the agent status of every current pane,
    /// and applies events until the subscription ends or must be renewed.
    pub fn run_once(&self, connector: &dyn Connector) -> SessionEnd {
        let mut started = false;
        let subscribed = match self.refresh_agents() {
            Ok(panes) => panes,
            Err(error) => {
                return SessionEnd {
                    started,
                    error: Some(error),
                };
            }
        };
        let mut failure = None;
        let result = subscribe(connector, &subscribed, |message| {
            if message == Message::Started {
                started = true;
            }
            if let Message::Event(event) = &message
                && let Err(error) = self.apply_worktree_event(event)
            {
                eprintln!("toodo: failed to apply herdr event {event:?}: {error}");
            }
            // Every message may change which pane runs which agent, so the statuses are
            // read again instead of patched from the event.
            match self.refresh_agents() {
                Ok(panes) if panes.is_subset(&subscribed) => Flow::Continue,
                Ok(_) => Flow::Resubscribe,
                Err(error) => {
                    failure = Some(error);
                    Flow::Resubscribe
                }
            }
        });
        let error = match result {
            Ok(()) => failure,
            Err(error) => Some(SyncError::Stream(error)),
        };
        SessionEnd { started, error }
    }

    /// Forgets all agent statuses, e.g. after herdr stopped.
    pub fn clear_agents(&self) {
        self.publish(Vec::new());
    }

    /// Reads the panes again, publishes the agent status of each task, and returns the
    /// IDs of the panes.
    fn refresh_agents(&self) -> Result<BTreeSet<String>, SyncError> {
        let panes = self.herdr.list_panes()?;
        let mut by_workspace: BTreeMap<&str, herdr::AgentStatus> = BTreeMap::new();
        for pane in &panes {
            let status = by_workspace
                .entry(pane.workspace_id.as_str())
                .or_insert(pane.status);
            *status = (*status).max(pane.status);
        }
        let mut statuses: Vec<TaskAgentStatus> = local_tasks(&lock(self.storage))?
            .into_iter()
            .filter_map(|task| {
                let status = by_workspace.get(task.herdr_workspace_id.as_deref()?)?;
                Some(TaskAgentStatus {
                    task_id: task.id.into(),
                    status: AgentStatus::from_herdr(*status)?,
                })
            })
            .collect();
        statuses.sort_by_key(|status| status.task_id.0);
        self.publish(statuses);
        Ok(panes.into_iter().map(|pane| pane.pane_id).collect())
    }

    fn publish(&self, statuses: Vec<TaskAgentStatus>) {
        let mut current = lock_statuses(self.statuses);
        if *current != statuses {
            *current = statuses;
            self.notifier.agent_statuses_changed(&current);
        }
    }

    fn apply_worktree_event(&self, event: &HerdrEvent) -> Result<(), StorageError> {
        let mut storage = lock(self.storage);
        let tasks = local_tasks(&storage)?;
        match event {
            HerdrEvent::WorktreeRemoved { workspace_id, .. } => {
                for task in tasks
                    .iter()
                    .filter(|task| task.herdr_workspace_id.as_ref() == Some(workspace_id))
                {
                    storage.clear_task_worktree(task.id)?;
                    self.notifier.tasks_changed(task.workspace_id);
                }
            }
            HerdrEvent::WorktreeOpened { workspace_id, path } => {
                let Ok(path) = WindowsPath::parse(path) else {
                    return Ok(());
                };
                for task in tasks.iter().filter(|task| {
                    task.herdr_workspace_id.as_ref() != Some(workspace_id)
                        && matches!(&task.worktree_path, Some(HostPath::Windows(worktree))
                            if worktree.as_str().eq_ignore_ascii_case(path.as_str()))
                }) {
                    storage.set_task_herdr_workspace(task.id, workspace_id)?;
                    self.notifier.tasks_changed(task.workspace_id);
                }
            }
            HerdrEvent::AgentsChanged => {}
        }
        Ok(())
    }
}

/// Tasks of workspaces on this machine, the only ones the local herdr server knows.
fn local_tasks(storage: &crate::storage::Storage) -> Result<Vec<model::Task>, StorageError> {
    let mut tasks = Vec::new();
    for workspace in storage.list_workspaces()? {
        if matches!(workspace.location, WorkspaceLocation::Local { .. }) {
            tasks.extend(storage.list_tasks(workspace.id)?);
        }
    }
    Ok(tasks)
}

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);

/// Starts the background thread that keeps the subscription to the local herdr server.
pub fn spawn(app: AppHandle) -> std::io::Result<()> {
    thread::Builder::new()
        .name("herdr-events".to_owned())
        .spawn(move || run(&app))
        .map(drop)
}

fn run(app: &AppHandle) {
    let storage = app.state::<StorageState>();
    let herdr = app.state::<HerdrState>();
    let statuses = app.state::<AgentStatusState>();
    let sync = EventSync {
        storage: &storage,
        herdr: &herdr,
        statuses: &statuses,
        notifier: app,
    };
    let mut delay = FIRST_RETRY;
    let mut last_error = String::new();
    loop {
        let end = match herdr.server_status() {
            Ok(ServerStatus {
                running: true,
                socket: Some(socket),
                ..
            }) => sync.run_once(&LocalSocket::new(&socket)),
            Ok(_) => SessionEnd {
                started: false,
                error: Some(SyncError::NotRunning),
            },
            Err(error) => SessionEnd {
                started: false,
                error: Some(error.into()),
            },
        };
        let Some(error) = end.error else {
            delay = FIRST_RETRY;
            continue;
        };
        // A subscription herdr accepted, or dropped for a reason a fresh pane list fixes,
        // is retried soon; a server that stays unreachable is retried less and less often.
        if end.started || matches!(&error, SyncError::Stream(error) if error.is_transient()) {
            delay = FIRST_RETRY;
        }
        sync.clear_agents();
        let message = error.to_string();
        if message != last_error {
            eprintln!("toodo: herdr events unavailable, retrying: {message}");
            last_error = message;
        }
        thread::sleep(delay);
        delay = (delay * 2).min(MAX_RETRY);
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::GitError;
    use crate::adapters::git::fake::FakeRunner;
    use crate::adapters::herdr_events::fake::FakeConnector;
    use crate::domain::model::{MachineLabel, TaskType};
    use crate::domain::naming::TaskCheckout;
    use crate::domain::path::PosixPath;
    use crate::storage::{NewTask, NewWorkspace, Storage};

    #[derive(Default)]
    struct Recorder {
        tasks_changed: Mutex<Vec<model::WorkspaceId>>,
        statuses: Mutex<Vec<Vec<TaskAgentStatus>>>,
    }

    impl Notifier for Recorder {
        fn tasks_changed(&self, workspace: model::WorkspaceId) {
            self.tasks_changed.lock().unwrap().push(workspace);
        }

        fn agent_statuses_changed(&self, statuses: &[TaskAgentStatus]) {
            self.statuses.lock().unwrap().push(statuses.to_vec());
        }
    }

    struct Fixture {
        storage: StorageState,
        statuses: AgentStatusState,
        recorder: Recorder,
        local: model::Task,
        remote: model::Task,
    }

    /// A started task in a local workspace (herdr workspace `w1`) and one in an SSH
    /// workspace whose remote herdr also calls its workspace `w1`.
    fn fixture() -> Fixture {
        let mut storage = Storage::open_in_memory().unwrap();
        let mut started = |location: WorkspaceLocation, path: HostPath| {
            let workspace = storage
                .create_workspace(&NewWorkspace {
                    name: "toodo".to_owned(),
                    location,
                    default_base_branch: "main".to_owned(),
                })
                .unwrap();
            let task = storage
                .create_task(&NewTask {
                    workspace_id: workspace.id,
                    task_type: TaskType::Feat,
                    title: "Add login".to_owned(),
                    description: String::new(),
                })
                .unwrap();
            let checkout = TaskCheckout {
                path,
                branch: format!("feat/add-login-{}", task.id),
            };
            storage.start_task(task.id, &checkout, "w1").unwrap()
        };
        let local = started(
            WorkspaceLocation::Local {
                path: WindowsPath::parse(r"C:\dev\toodo").unwrap(),
            },
            HostPath::Windows(WindowsPath::parse(r"D:\wt\toodo\feat-add-login-1").unwrap()),
        );
        let remote = started(
            WorkspaceLocation::Ssh {
                machine: MachineLabel::parse("devbox").unwrap(),
                path: PosixPath::parse("/srv/toodo").unwrap(),
            },
            HostPath::Posix(PosixPath::parse("/wt/toodo/feat-add-login-2").unwrap()),
        );
        Fixture {
            storage: Mutex::new(storage),
            statuses: Mutex::default(),
            recorder: Recorder::default(),
            local,
            remote,
        }
    }

    fn herdr(pane_lists: &[&str]) -> Herdr {
        let results = pane_lists.iter().map(|panes| {
            Ok(format!(
                r#"{{"id":"cli:pane:list","result":{{"panes":[{panes}]}}}}"#
            ))
        });
        Herdr::new(Box::new(Arc::new(FakeRunner::new(
            results.collect::<Vec<Result<String, GitError>>>(),
        ))))
    }

    fn pane(pane_id: &str, workspace_id: &str, status: &str) -> String {
        format!(
            r#"{{"agent_status":"{status}","pane_id":"{pane_id}","workspace_id":"{workspace_id}"}}"#
        )
    }

    const STARTED: &str = r#"{"id":"toodo:events","result":{"type":"subscription_started"}}"#;

    fn sync<'a>(fixture: &'a Fixture, herdr: &'a Herdr) -> EventSync<'a> {
        EventSync {
            storage: &fixture.storage,
            herdr,
            statuses: &fixture.statuses,
            notifier: &fixture.recorder,
        }
    }

    fn task(fixture: &Fixture, id: model::TaskId) -> model::Task {
        lock(&fixture.storage).get_task(id).unwrap().unwrap()
    }

    fn statuses(entries: &[(model::TaskId, AgentStatus)]) -> Vec<TaskAgentStatus> {
        entries
            .iter()
            .map(|(id, status)| TaskAgentStatus {
                task_id: (*id).into(),
                status: *status,
            })
            .collect()
    }

    #[test]
    fn removed_worktrees_are_forgotten_on_local_tasks_only() {
        let fixture = fixture();
        let empty = String::new();
        let herdr = herdr(&[&empty, &empty, &empty]);
        let removed = r#"{"data":{"forced":false,"type":"worktree_removed","workspace_id":"w1","worktree":{"path":"D:\\wt\\toodo\\feat-add-login-1"}},"event":"worktree_removed"}"#;
        let connector = FakeConnector::new([Ok(format!("{STARTED}\n{removed}\n"))]);
        let end = sync(&fixture, &herdr).run_once(&connector);
        assert!(end.started);
        assert!(matches!(
            end.error,
            Some(SyncError::Stream(EventStreamError::Closed))
        ));
        let local = task(&fixture, fixture.local.id);
        assert_eq!(local.worktree_path, None);
        assert_eq!(local.herdr_workspace_id, None);
        assert_eq!(local.branch, fixture.local.branch);
        let remote = task(&fixture, fixture.remote.id);
        assert_eq!(remote.herdr_workspace_id.as_deref(), Some("w1"));
        assert_eq!(
            *fixture.recorder.tasks_changed.lock().unwrap(),
            [fixture.local.workspace_id]
        );
    }

    #[test]
    fn reopened_worktrees_adopt_the_new_herdr_workspace() {
        let fixture = fixture();
        let empty = String::new();
        let herdr = herdr(&[&empty, &empty, &empty]);
        let opened = r#"{"data":{"already_open":false,"type":"worktree_opened","workspace":{"workspace_id":"w9"},"worktree":{"path":"d:/wt/toodo/feat-add-login-1/"}},"event":"worktree_opened"}"#;
        let connector = FakeConnector::new([Ok(format!("{STARTED}\n{opened}\n"))]);
        sync(&fixture, &herdr).run_once(&connector);
        let local = task(&fixture, fixture.local.id);
        assert_eq!(local.herdr_workspace_id.as_deref(), Some("w9"));
        assert_eq!(local.worktree_path, fixture.local.worktree_path);
        assert_eq!(
            task(&fixture, fixture.remote.id)
                .herdr_workspace_id
                .as_deref(),
            Some("w1")
        );
        assert_eq!(
            *fixture.recorder.tasks_changed.lock().unwrap(),
            [fixture.local.workspace_id]
        );
    }

    #[test]
    fn agent_statuses_summarize_the_panes_of_each_task_workspace() {
        let fixture = fixture();
        let before = [pane("w1:p1", "w1", "idle"), pane("w7:p1", "w7", "working")].join(",");
        let after = [
            pane("w1:p1", "w1", "working"),
            pane("w1:p2", "w1", "blocked"),
            pane("w1:p3", "w1", "unknown"),
        ]
        .join(",");
        let herdr = herdr(&[&before, &before, &after]);
        let status = r#"{"data":{"agent":"claude","agent_status":"blocked","pane_id":"w1:p2","workspace_id":"w1"},"event":"pane.agent_status_changed"}"#;
        let connector = FakeConnector::new([Ok(format!("{STARTED}\n{status}\n"))]);
        let end = sync(&fixture, &herdr).run_once(&connector);
        // The new panes of `w1` need their own subscription.
        assert!(end.started);
        assert!(end.error.is_none());
        let id = fixture.local.id;
        assert_eq!(
            *fixture.recorder.statuses.lock().unwrap(),
            [
                statuses(&[(id, AgentStatus::Idle)]),
                statuses(&[(id, AgentStatus::Blocked)]),
            ]
        );
        assert_eq!(
            list_statuses(&fixture),
            statuses(&[(id, AgentStatus::Blocked)])
        );
        let request = &connector.requests.lock().unwrap()[0];
        assert!(request.contains(r#""pane_id":"w1:p1""#));
        assert!(request.contains(r#""pane_id":"w7:p1""#));
    }

    fn list_statuses(fixture: &Fixture) -> Vec<TaskAgentStatus> {
        lock_statuses(&fixture.statuses).clone()
    }

    #[test]
    fn a_failed_subscription_keeps_nothing_stale() {
        let fixture = fixture();
        let working = pane("w1:p1", "w1", "working");
        let herdr = herdr(&[&working]);
        let connector =
            FakeConnector::new([Err(io::Error::new(io::ErrorKind::NotFound, "no socket"))]);
        let sync = sync(&fixture, &herdr);
        let end = sync.run_once(&connector);
        assert!(!end.started);
        assert!(matches!(
            end.error,
            Some(SyncError::Stream(EventStreamError::Io { .. }))
        ));
        sync.clear_agents();
        assert_eq!(list_statuses(&fixture), []);
        assert_eq!(fixture.recorder.statuses.lock().unwrap().len(), 2);
    }
}
