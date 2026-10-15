//! How agents connect to Toodo's MCP server.

use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::mcp::ServerStatus;

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

#[tauri::command]
#[specta::specta]
pub fn mcp_server_info(status: State<'_, ServerStatus>) -> McpServerInfo {
    status.inner().into()
}
