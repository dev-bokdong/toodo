//! The user's OpenSSH client config, shared with `ssh.exe` and herdr, and directory
//! listings of remote machines for the folder browser.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use super::git::{GitError, Runner, shell_quote, ssh_invocation};
use crate::domain::model::MachineLabel;
use crate::domain::path::PosixPath;
use crate::domain::ssh_config::{self, SshHost, SshHostError};

#[derive(Debug)]
pub enum SshConfigError {
    Io { path: PathBuf, error: io::Error },
    Host(SshHostError),
}

impl fmt::Display for SshConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "failed to access {}: {error}", path.display()),
            Self::Host(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SshConfigError {}

/// `~/.ssh/config`; `%USERPROFILE%\.ssh\config` on Windows.
pub struct SshConfigFile {
    path: PathBuf,
}

impl SshConfigFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The config in the user's home directory, or `None` when the home directory is
    /// unknown.
    pub fn in_home() -> Option<Self> {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .filter(|home| !home.is_empty())?;
        Some(Self::new(PathBuf::from(home).join(".ssh").join("config")))
    }

    /// Host aliases of the config, read on every call so external edits show up. A
    /// missing file has no hosts.
    pub fn host_aliases(&self) -> Result<Vec<String>, SshConfigError> {
        Ok(ssh_config::host_aliases(&self.read()?))
    }

    /// Appends a `Host` block, creating the file and its directory when missing.
    pub fn add_host(&self, host: &SshHost) -> Result<(), SshConfigError> {
        let block = host
            .block_to_append(&self.read()?)
            .map_err(SshConfigError::Host)?;
        let io_error = |error| SshConfigError::Io {
            path: self.path.clone(),
            error,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(io_error)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(block.as_bytes()))
            .map_err(io_error)
    }

    fn read(&self) -> Result<String, SshConfigError> {
        match fs::read_to_string(&self.path) {
            Ok(config) => Ok(config),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(SshConfigError::Io {
                path: self.path.clone(),
                error,
            }),
        }
    }
}

/// Subdirectories of one directory on a remote machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDirectory {
    /// Absolute path of the listed directory as resolved by the remote shell.
    pub path: PosixPath,
    /// Names of subdirectories, hidden ones included, sorted case-insensitively.
    pub directories: Vec<String>,
}

pub struct Ssh {
    runner: Box<dyn Runner>,
}

impl Ssh {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner }
    }

    /// Lists the subdirectories of `path` on `machine`, or of the login directory when
    /// `path` is `None`. Runs a POSIX `sh` script so that it works whatever the login
    /// shell of the remote user is.
    pub fn list_directories(
        &self,
        machine: &MachineLabel,
        path: Option<&PosixPath>,
    ) -> Result<RemoteDirectory, GitError> {
        let cd = path
            .map(|path| format!("cd -- {} || exit 1; ", shell_quote(path.as_str())))
            .unwrap_or_default();
        let script = format!(
            "{cd}pwd; for entry in * .*; do [ -d \"$entry\" ] && printf '%s\\n' \"$entry\"; done; exit 0"
        );
        let invocation = ssh_invocation(machine, format!("sh -c {}", shell_quote(&script)));
        let output = self.runner.run(&invocation)?;
        let mut lines = output.lines();
        let unexpected = || GitError::UnexpectedOutput {
            command: format!("ssh {machine} ls"),
            output: output.clone(),
        };
        let path = lines
            .next()
            .and_then(|line| PosixPath::parse(line.trim_end_matches('\r')).ok())
            .ok_or_else(unexpected)?;
        let mut directories: Vec<String> = lines
            .map(|line| line.trim_end_matches('\r'))
            .filter(|name| !matches!(*name, "" | "." | ".."))
            .map(str::to_owned)
            .collect();
        directories.sort_by_key(|name| name.to_lowercase());
        directories.dedup();
        Ok(RemoteDirectory { path, directories })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::fake::{FakeRunner, failed};

    fn devbox() -> MachineLabel {
        MachineLabel::parse("devbox").unwrap()
    }

    #[test]
    fn list_directories_runs_a_quoted_script_and_sorts_the_names() {
        let runner = Arc::new(FakeRunner::new([Ok(
            "/home/me/it's\nsrc\n.\n..\n.config\nDocs\napp\n".to_owned(),
        )]));
        let ssh = Ssh::new(Box::new(runner.clone()));
        let listing = ssh.list_directories(&devbox(), None);
        let invocation = runner.invocations.lock().unwrap()[0].clone();
        assert_eq!(invocation.program, "ssh");
        assert_eq!(
            invocation.args[..4],
            ["-o", "BatchMode=yes", "--", "devbox"]
        );
        assert_eq!(
            invocation.args[4],
            r#"sh -c 'pwd; for entry in * .*; do [ -d "$entry" ] && printf '\''%s\n'\'' "$entry"; done; exit 0'"#
        );
        assert_eq!(
            listing.unwrap(),
            RemoteDirectory {
                path: PosixPath::parse("/home/me/it's").unwrap(),
                directories: [".config", "app", "Docs", "src"]
                    .map(str::to_owned)
                    .to_vec(),
            }
        );
    }

    /// Runs the remote command the way sshd does, through a local POSIX shell.
    #[cfg(unix)]
    #[test]
    fn remote_script_lists_directories_of_a_quoted_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("it's here");
        for sub in ["b dir", ".hidden", "a"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        fs::write(root.join("file.txt"), "").unwrap();
        let runner = Arc::new(FakeRunner::new([Ok("/\n".to_owned())]));
        let ssh = Ssh::new(Box::new(runner.clone()));
        let path = PosixPath::parse(root.to_str().unwrap()).unwrap();
        ssh.list_directories(&devbox(), Some(&path)).unwrap();
        let remote = runner.invocations.lock().unwrap()[0].args[4].clone();
        let output = std::process::Command::new("sh")
            .args(["-c", &remote])
            .output()
            .unwrap();
        let real = Ssh::new(Box::new(Arc::new(FakeRunner::new([Ok(
            String::from_utf8(output.stdout).unwrap(),
        )]))));
        let listing = real.list_directories(&devbox(), None).unwrap();
        assert_eq!(
            listing.path.as_str(),
            root.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(listing.directories, [".hidden", "a", "b dir"]);

        let missing = PosixPath::parse("/no/such/dir-for-toodo").unwrap();
        let runner = Arc::new(FakeRunner::new([Ok(String::new())]));
        Ssh::new(Box::new(runner.clone()))
            .list_directories(&devbox(), Some(&missing))
            .unwrap_err();
        let remote = runner.invocations.lock().unwrap()[0].args[4].clone();
        let status = std::process::Command::new("sh")
            .args(["-c", &remote])
            .output()
            .unwrap()
            .status;
        assert!(!status.success());
    }

    #[test]
    fn list_directories_reports_failures_and_garbage() {
        let runner = Arc::new(FakeRunner::new([
            Err(failed()),
            Ok("relative\n".to_owned()),
        ]));
        let ssh = Ssh::new(Box::new(runner.clone()));
        assert_eq!(ssh.list_directories(&devbox(), None), Err(failed()));
        assert!(matches!(
            ssh.list_directories(&devbox(), None),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn config_file_lists_and_appends_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let config = SshConfigFile::new(dir.path().join(".ssh").join("config"));
        assert!(config.host_aliases().unwrap().is_empty());
        config
            .add_host(&SshHost::new("devbox", "10.0.0.5", Some("me"), None).unwrap())
            .unwrap();
        config
            .add_host(&SshHost::new("gpu", "gpu.lan", None, Some(2222)).unwrap())
            .unwrap();
        assert_eq!(config.host_aliases().unwrap(), ["devbox", "gpu"]);
        assert_eq!(
            fs::read_to_string(dir.path().join(".ssh").join("config")).unwrap(),
            "Host devbox\n    HostName 10.0.0.5\n    User me\n\nHost gpu\n    HostName gpu.lan\n    Port 2222\n"
        );
        assert!(matches!(
            config.add_host(&SshHost::new("gpu", "x", None, None).unwrap()),
            Err(SshConfigError::Host(SshHostError::AlreadyExists(_)))
        ));
    }
}
