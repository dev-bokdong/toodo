//! How agents connect to Toodo's MCP server.

use super::State;
use serde::Serialize;
use specta::Type;

use crate::mcp::ServerStatus;
use crate::mcp::tunnels::{TunnelState, TunnelStatus, TunnelsState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub url: String,
    /// Bearer token agents send in the `Authorization` header.
    pub token: Option<String>,
    /// Why the server is not running, e.g. its port is taken.
    pub error: Option<String>,
}

impl From<&ServerStatus> for McpServerInfo {
    fn from(status: &ServerStatus) -> Self {
        Self {
            url: status.url.clone(),
            token: status.token.clone(),
            error: status.error.clone(),
        }
    }
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn mcp_server_info(status: State<'_, ServerStatus>) -> McpServerInfo {
    McpServerInfo::from(&*status)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpTunnelState {
    Connecting,
    Open,
    Retrying,
}

/// The reverse tunnel that lets agents on one SSH machine reach the MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpTunnel {
    pub machine: String,
    pub state: McpTunnelState,
    /// Why the last tunnel ended, until a new one is open.
    pub error: Option<String>,
}

impl From<TunnelStatus> for McpTunnel {
    fn from(status: TunnelStatus) -> Self {
        Self {
            machine: status.machine.as_str().to_owned(),
            state: match status.state {
                TunnelState::Connecting => McpTunnelState::Connecting,
                TunnelState::Open => McpTunnelState::Open,
                TunnelState::Retrying => McpTunnelState::Retrying,
            },
            error: status.error,
        }
    }
}

#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_mcp_tunnels(tunnels: State<'_, TunnelsState>) -> Vec<McpTunnel> {
    let tunnels = tunnels
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tunnels.statuses().into_iter().map(Into::into).collect()
}
