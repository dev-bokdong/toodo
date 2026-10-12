//! Commands for the machines that host workspaces: SSH hosts from the user's SSH config
//! and the remote folder browser.

use tauri::State;

use super::types::{self, NewSshHost, RemoteDirectory};
use super::{CommandError, CommandResult, SshConfigState, SshState};
use crate::adapters::ssh::SshConfigFile;
use crate::domain::model::MachineLabel;
use crate::domain::path::PosixPath;
use crate::domain::ssh_config::SshHost;

fn config_file(config: &SshConfigState) -> CommandResult<&SshConfigFile> {
    config.as_ref().ok_or_else(|| {
        CommandError::internal("the home directory that holds the SSH config is unknown")
    })
}

/// Host aliases from `~/.ssh/config`, re-read on every call.
#[tauri::command]
#[specta::specta]
pub fn list_ssh_hosts(config: State<'_, SshConfigState>) -> CommandResult<Vec<String>> {
    Ok(config_file(&config)?.host_aliases()?)
}

/// Appends a `Host` block to `~/.ssh/config` and returns the updated host aliases.
#[tauri::command]
#[specta::specta]
pub fn add_ssh_host(
    config: State<'_, SshConfigState>,
    host: NewSshHost,
) -> CommandResult<Vec<String>> {
    let host: SshHost = host.try_into()?;
    let file = config_file(&config)?;
    file.add_host(&host)?;
    Ok(file.host_aliases()?)
}

/// Subdirectories of `path` on `machine`, or of the login directory when `path` is
/// absent. ssh runs on a blocking thread because connecting can take seconds.
#[tauri::command]
#[specta::specta]
pub async fn list_remote_directories(
    ssh: State<'_, SshState>,
    machine: String,
    path: Option<String>,
) -> CommandResult<RemoteDirectory> {
    let machine = MachineLabel::parse(&machine)
        .map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let path = path
        .filter(|path| !path.trim().is_empty())
        .map(|path| PosixPath::parse(path.trim()))
        .transpose()
        .map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let ssh = SshState::clone(&ssh);
    let listing =
        tauri::async_runtime::spawn_blocking(move || ssh.list_directories(&machine, path.as_ref()))
            .await
            .map_err(|error| {
                CommandError::internal(format!("directory listing panicked: {error}"))
            })??;
    Ok(types::RemoteDirectory::from(&listing))
}
