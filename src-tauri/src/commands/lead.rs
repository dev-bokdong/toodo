//! The lead: an agent that `toodo-server` keeps running in a herdr pane of its own
//! workspace and that turns requests into tasks. The desktop app has no lead, so its
//! status there is always [`LeadState::Disabled`].

use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use specta::Type;

use super::State;
use super::types::{AgentStatus, Timestamp, WorkspaceId};

/// What the server is doing with the lead.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LeadState {
    /// No lead: the desktop app, or a server started with `TOODO_LEAD=off`.
    #[default]
    Disabled,
    /// The agent command was typed into the pane and has not shown up yet.
    Starting,
    Running,
    /// The agent exited; it is started again once its backoff has passed.
    Restarting,
    /// The lead cannot run: herdr or git failed, or the agent keeps exiting right after
    /// it starts. The server keeps trying.
    Failed,
}

/// The lead's workspace, pane, and state, for the chat panel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeadStatus {
    pub state: LeadState,
    /// The Toodo workspace the lead works in.
    pub workspace_id: Option<WorkspaceId>,
    pub herdr_workspace_id: Option<String>,
    pub pane_id: Option<String>,
    /// The agent herdr recognized in the pane, e.g. `omo`.
    pub agent: Option<String>,
    pub agent_status: Option<AgentStatus>,
    /// How often the server started the agent again since the server started.
    pub restarts: u32,
    /// When the running agent was started, or first seen running.
    pub started_at: Option<Timestamp>,
    /// Why the lead is failing or restarting.
    pub error: Option<String>,
}

pub type LeadStatusState = Mutex<LeadStatus>;

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn get_lead(lead: State<'_, LeadStatusState>) -> LeadStatus {
    lead.lock().unwrap_or_else(PoisonError::into_inner).clone()
}
