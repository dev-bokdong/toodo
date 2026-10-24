//! Commands for the machines that host workspaces: machines saved in herdr, SSH hosts from
//! the user's SSH config, and the remote folder browser.

use super::State;

use std::io;
use std::path::Path;

use super::types::{self, HerdrMachine, Machine, NewSshHost, RemoteDirectory};
use super::{CommandError, CommandResult, HerdrState, SshConfigState, SshState};
use crate::adapters::herdr::Herdr;
use crate::adapters::ssh::SshConfigFile;
use crate::domain::model::MachineLabel;
use crate::domain::path::PosixPath;
use crate::domain::ssh_config::SshHost;

fn config_file(config: &SshConfigState) -> CommandResult<&SshConfigFile> {
    config.as_ref().ok_or_else(|| {
        CommandError::internal("the home directory that holds the SSH config is unknown")
    })
}

/// Machines saved in herdr; remote workspaces can only live on these.
#[cfg_attr(feature = "desktop", tauri::command(async))]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_herdr_machines(herdr: State<'_, HerdrState>) -> CommandResult<Vec<HerdrMachine>> {
    Ok(herdr
        .list_machines()?
        .iter()
        .map(HerdrMachine::from)
        .collect())
}

/// Runs `herdr machine add <host>` in a terminal window, where herdr asks before it
/// installs or replaces the remote herdr, and returns the saved machines once the window
/// closes. Fails when herdr saved no machine for the host. The server has no terminal
/// window to run it in and refuses it instead.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub async fn add_herdr_machine(
    herdr: State<'_, HerdrState>,
    host: String,
) -> CommandResult<Vec<HerdrMachine>> {
    let host = MachineLabel::parse(&host)
        .map_err(|error| CommandError::invalid_input(error.to_string()))?;
    let herdr = HerdrState::clone(&herdr);
    tokio::task::spawn_blocking(move || add_machine(&herdr, &host))
        .await
        .map_err(|error| CommandError::internal(format!("herdr machine add panicked: {error}")))?
}

#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn add_machine(herdr: &Herdr, host: &MachineLabel) -> CommandResult<Vec<HerdrMachine>> {
    herdr.add_machine(host)?;
    let machines = herdr.list_machines()?;
    if !machines
        .iter()
        .any(|machine| machine.target == host.as_str())
    {
        return Err(CommandError::rejected(format!(
            "herdr saved no machine for SSH host '{host}'; the setup in the terminal was \
             cancelled or failed"
        )));
    }
    Ok(machines.iter().map(HerdrMachine::from).collect())
}

/// Host aliases from `~/.ssh/config`, re-read on every call.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_ssh_hosts(config: State<'_, SshConfigState>) -> CommandResult<Vec<String>> {
    Ok(config_file(&config)?.host_aliases()?)
}

/// Appends a `Host` block to `~/.ssh/config` and returns the updated host aliases.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
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
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
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
        tokio::task::spawn_blocking(move || ssh.list_directories(&machine, path.as_ref()))
            .await
            .map_err(|error| {
                CommandError::internal(format!("directory listing panicked: {error}"))
            })??;
    Ok(types::RemoteDirectory::from(&listing))
}

/// Subdirectories of `path` on `machine`, or of the home directory when `path` is absent.
/// On the server, "Local" is the server, so `local` lists the server's own directories;
/// the desktop app picks local folders with the system folder dialog and refuses it.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub async fn list_directories(
    ssh: State<'_, SshState>,
    machine: Machine,
    path: Option<String>,
) -> CommandResult<RemoteDirectory> {
    match machine {
        Machine::Ssh { label } => list_remote_directories(ssh, label, path).await,
        Machine::Local => list_server_directories(path).await,
    }
}

#[cfg(feature = "server")]
async fn list_server_directories(path: Option<String>) -> CommandResult<RemoteDirectory> {
    let path = path
        .filter(|path| !path.trim().is_empty())
        .map(|path| PosixPath::parse(path.trim()))
        .transpose()
        .map_err(|error| CommandError::invalid_input(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let dir = match path {
            Some(path) => std::path::PathBuf::from(path.as_str()),
            None => std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .ok_or_else(|| CommandError::internal("HOME is not set on the server"))?,
        };
        local_directories(&dir)
    })
    .await
    .map_err(|error| CommandError::internal(format!("directory listing panicked: {error}")))?
}

#[cfg(not(feature = "server"))]
async fn list_server_directories(_path: Option<String>) -> CommandResult<RemoteDirectory> {
    Err(CommandError::rejected(
        "the desktop app picks local folders with the folder dialog",
    ))
}

/// Subdirectories of `dir` on this machine, hidden ones included, sorted like a remote
/// listing; symbolic links to directories count as directories. `dir` is resolved to an
/// absolute path without `..` or links first.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
fn local_directories(dir: &Path) -> CommandResult<RemoteDirectory> {
    let failed = |error: io::Error| {
        let message = format!("cannot list {}: {error}", dir.display());
        match error.kind() {
            io::ErrorKind::NotFound => CommandError::not_found(message),
            io::ErrorKind::NotADirectory => CommandError::invalid_input(message),
            io::ErrorKind::PermissionDenied => CommandError::rejected(message),
            _ => CommandError::internal(message),
        }
    };
    let resolved = dir.canonicalize().map_err(failed)?;
    let path = resolved
        .to_str()
        .and_then(|path| PosixPath::parse(path).ok())
        .ok_or_else(|| {
            CommandError::invalid_input(format!("{} is not a POSIX path", resolved.display()))
        })?;
    let mut directories: Vec<String> = std::fs::read_dir(&resolved)
        .map_err(failed)?
        .filter_map(Result::ok)
        .filter(|entry| std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    directories.sort_by_key(|name| name.to_lowercase());
    Ok(RemoteDirectory {
        path: path.as_str().to_owned(),
        directories,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::GitError;
    use crate::adapters::git::fake::FakeRunner;
    use crate::commands::CommandErrorKind;

    const SAVED: &str = r#"[{"id":"m1","label":"devbox","target":"devbox","session":"default",
        "enabled":true,"selected":false}]"#;

    fn herdr(results: Vec<Result<String, GitError>>) -> (Herdr, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (Herdr::new(Box::new(runner.clone())), runner)
    }

    fn devbox() -> MachineLabel {
        MachineLabel::parse("devbox").unwrap()
    }

    #[test]
    fn added_machines_are_returned_with_the_saved_list() {
        let (herdr, runner) = herdr(vec![Ok(String::new()), Ok(SAVED.to_owned())]);
        let machines = add_machine(&herdr, &devbox()).unwrap();
        assert_eq!(machines.len(), 1);
        assert_eq!(machines[0].id, "m1");
        assert_eq!(machines[0].target, "devbox");
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(invocations[0].args, ["machine", "add", "devbox"]);
        assert_eq!(invocations[1].args, ["machine", "list", "--json"]);
    }

    #[cfg(unix)]
    #[test]
    fn local_listings_hold_directories_and_links_to_them_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("it's here");
        for sub in ["b dir", ".hidden", "A"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "").unwrap();
        std::os::unix::fs::symlink(root.join("A"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("broken")).unwrap();

        let listing = local_directories(&root.join("b dir/..")).unwrap();

        assert_eq!(listing.path, root.canonicalize().unwrap().to_str().unwrap());
        assert_eq!(listing.directories, [".hidden", "A", "b dir", "link"]);
        let kind = |path: &Path| local_directories(path).unwrap_err().kind;
        assert_eq!(kind(&root.join("missing")), CommandErrorKind::NotFound);
        assert_eq!(kind(&root.join("file.txt")), CommandErrorKind::InvalidInput);
    }

    #[test]
    fn a_cancelled_setup_is_rejected() {
        let (herdr, _) = herdr(vec![Ok(String::new()), Ok("[]".to_owned())]);
        let error = add_machine(&herdr, &devbox()).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
    }

    #[test]
    fn a_failed_setup_is_reported_without_listing() {
        let (herdr, runner) = herdr(vec![Err(GitError::Failed {
            command: "herdr machine add devbox".to_owned(),
            stderr: "exited with exit status: 1".to_owned(),
        })]);
        let error = add_machine(&herdr, &devbox()).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::External);
        assert_eq!(runner.invocations.lock().unwrap().len(), 1);
    }
}
