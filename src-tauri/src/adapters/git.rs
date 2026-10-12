//! Runs git for a workspace: `git.exe` for local workspaces and `ssh.exe <machine> git ...`
//! for workspaces on a saved herdr SSH machine.

use std::fmt;
use std::process::{Command, Stdio};

use crate::domain::model::WorkspaceLocation;
use crate::domain::path::{PosixPath, WindowsPath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: &'static str,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// The process could not be started, e.g. `git` or `ssh` is not on `PATH`.
    Spawn { program: String, message: String },
    /// The process exited with a non-zero status.
    Failed { command: String, stderr: String },
    /// The process succeeded but printed something the app cannot use.
    UnexpectedOutput { command: String, output: String },
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, message } => write!(f, "failed to run {program}: {message}"),
            Self::Failed { command, stderr } => write!(f, "`{command}` failed: {stderr}"),
            Self::UnexpectedOutput { command, output } => {
                write!(f, "`{command}` printed unexpected output: {output:?}")
            }
        }
    }
}

impl std::error::Error for GitError {}

/// Runs an invocation and returns its standard output.
pub trait Runner: Send + Sync {
    fn run(&self, invocation: &Invocation) -> Result<String, GitError>;
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, invocation: &Invocation) -> Result<String, GitError> {
        let mut command = Command::new(invocation.program);
        command
            .args(&invocation.args)
            // Fail instead of waiting for input on a terminal the user cannot see.
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let output = command.output().map_err(|error| GitError::Spawn {
            program: invocation.program.to_owned(),
            message: error.to_string(),
        })?;
        if !output.status.success() {
            return Err(GitError::Failed {
                command: describe(invocation),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

fn describe(invocation: &Invocation) -> String {
    std::iter::once(invocation.program)
        .chain(invocation.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes one word for the POSIX shell that runs an SSH remote command.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

pub struct Git {
    runner: Box<dyn Runner>,
}

impl Git {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner }
    }

    /// Clones `url` into the location's path, which must not exist or be empty.
    pub fn clone_repository(&self, url: &str, target: &WorkspaceLocation) -> Result<(), GitError> {
        let path = location_path(target);
        self.run(target, None, &["clone", "--", url, path])?;
        Ok(())
    }

    /// Returns the root of the repository that contains the location's path. Fails when
    /// the path is not inside a git repository.
    pub fn repository_root(
        &self,
        location: &WorkspaceLocation,
    ) -> Result<WorkspaceLocation, GitError> {
        let args = ["rev-parse", "--show-toplevel"];
        let output = self.run(location, Some(location_path(location)), &args)?;
        let root = output.trim();
        let unexpected = || GitError::UnexpectedOutput {
            command: format!("git {}", args.join(" ")),
            output: root.to_owned(),
        };
        Ok(match location {
            WorkspaceLocation::Local { .. } => WorkspaceLocation::Local {
                path: WindowsPath::parse(root).map_err(|_| unexpected())?,
            },
            WorkspaceLocation::Ssh { machine, .. } => WorkspaceLocation::Ssh {
                machine: machine.clone(),
                path: PosixPath::parse(root).map_err(|_| unexpected())?,
            },
        })
    }

    /// Returns the branch `origin/HEAD` points to, or else the checked-out branch.
    /// Returns `None` when neither exists, e.g. without a remote on a detached HEAD.
    pub fn default_branch(&self, repo: &WorkspaceLocation) -> Result<Option<String>, GitError> {
        let path = Some(location_path(repo));
        let symbolic_ref =
            |name| self.run(repo, path, &["symbolic-ref", "--quiet", "--short", name]);
        let branch = match symbolic_ref("refs/remotes/origin/HEAD") {
            Ok(remote_head) => {
                let remote_head = remote_head.trim();
                remote_head
                    .strip_prefix("origin/")
                    .unwrap_or(remote_head)
                    .to_owned()
            }
            Err(GitError::Failed { .. }) => match symbolic_ref("HEAD") {
                Ok(head) => head.trim().to_owned(),
                Err(GitError::Failed { .. }) => return Ok(None),
                Err(error) => return Err(error),
            },
            Err(error) => return Err(error),
        };
        Ok(Some(branch).filter(|branch| !branch.is_empty()))
    }

    /// Runs `git [-C <dir>] <args>` on the machine that hosts `location`.
    fn run(
        &self,
        location: &WorkspaceLocation,
        dir: Option<&str>,
        args: &[&str],
    ) -> Result<String, GitError> {
        let mut git_args: Vec<&str> = Vec::new();
        if let Some(dir) = dir {
            git_args.extend(["-C", dir]);
        }
        git_args.extend(args);
        let invocation = match location {
            WorkspaceLocation::Local { .. } => Invocation {
                program: "git",
                args: git_args.iter().map(|arg| (*arg).to_owned()).collect(),
            },
            WorkspaceLocation::Ssh { machine, .. } => {
                let remote = std::iter::once("git".to_owned())
                    .chain(git_args.iter().map(|arg| shell_quote(arg)))
                    .collect::<Vec<_>>()
                    .join(" ");
                Invocation {
                    program: "ssh",
                    args: vec![
                        "-o".to_owned(),
                        "BatchMode=yes".to_owned(),
                        "--".to_owned(),
                        machine.as_str().to_owned(),
                        remote,
                    ],
                }
            }
        };
        self.runner.run(&invocation)
    }
}

fn location_path(location: &WorkspaceLocation) -> &str {
    match location {
        WorkspaceLocation::Local { path } => path.as_str(),
        WorkspaceLocation::Ssh { path, .. } => path.as_str(),
    }
}

#[cfg(test)]
pub mod fake {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// Returns scripted results in order and records every invocation.
    #[derive(Default)]
    pub struct FakeRunner {
        pub results: Mutex<VecDeque<Result<String, GitError>>>,
        pub invocations: Mutex<Vec<Invocation>>,
    }

    impl FakeRunner {
        pub fn new(results: impl IntoIterator<Item = Result<String, GitError>>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                invocations: Mutex::default(),
            }
        }
    }

    impl Runner for std::sync::Arc<FakeRunner> {
        fn run(&self, invocation: &Invocation) -> Result<String, GitError> {
            self.invocations.lock().unwrap().push(invocation.clone());
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected git invocation")
        }
    }

    pub fn failed() -> GitError {
        GitError::Failed {
            command: "git".to_owned(),
            stderr: "fatal".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::fake::{FakeRunner, failed};
    use super::*;
    use crate::domain::model::MachineLabel;

    fn git(results: Vec<Result<String, GitError>>) -> (Git, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (Git::new(Box::new(runner.clone())), runner)
    }

    fn local(path: &str) -> WorkspaceLocation {
        WorkspaceLocation::Local {
            path: WindowsPath::parse(path).unwrap(),
        }
    }

    fn remote(path: &str) -> WorkspaceLocation {
        WorkspaceLocation::Ssh {
            machine: MachineLabel::parse("devbox").unwrap(),
            path: PosixPath::parse(path).unwrap(),
        }
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn local_clone_runs_git_directly() {
        let (git, runner) = git(vec![Ok(String::new())]);
        git.clone_repository("https://example.com/a.git", &local(r"C:\dev\a"))
            .unwrap();
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            Invocation {
                program: "git",
                args: args(&["clone", "--", "https://example.com/a.git", r"C:\dev\a"]),
            }
        );
    }

    #[test]
    fn remote_clone_quotes_the_remote_command() {
        let (git, runner) = git(vec![Ok(String::new())]);
        git.clone_repository("git@host:o/a.git", &remote("/srv/it's"))
            .unwrap();
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            Invocation {
                program: "ssh",
                args: args(&[
                    "-o",
                    "BatchMode=yes",
                    "--",
                    "devbox",
                    r"git 'clone' '--' 'git@host:o/a.git' '/srv/it'\''s'",
                ]),
            }
        );
    }

    #[test]
    fn repository_root_is_parsed_for_the_machine() {
        let (git, runner) = git(vec![Ok("C:/dev/a\n".to_owned()), Ok("/srv/a\n".to_owned())]);
        assert_eq!(
            git.repository_root(&local(r"C:\dev\a\src")).unwrap(),
            local(r"C:\dev\a")
        );
        assert_eq!(
            git.repository_root(&remote("/srv/a/src")).unwrap(),
            remote("/srv/a")
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[0].args,
            args(&["-C", r"C:\dev\a\src", "rev-parse", "--show-toplevel"])
        );
    }

    #[test]
    fn repository_root_rejects_unparseable_output() {
        let (git, _) = git(vec![Ok("/c/dev/a\n".to_owned())]);
        assert!(matches!(
            git.repository_root(&local(r"C:\dev\a")),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn default_branch_prefers_origin_head() {
        let (git, _) = git(vec![Ok("origin/develop\n".to_owned())]);
        assert_eq!(
            git.default_branch(&local(r"C:\dev\a")).unwrap(),
            Some("develop".to_owned())
        );
    }

    #[test]
    fn default_branch_falls_back_to_the_checked_out_branch() {
        let (git, runner) = git(vec![Err(failed()), Ok("main\n".to_owned())]);
        assert_eq!(
            git.default_branch(&local(r"C:\dev\a")).unwrap(),
            Some("main".to_owned())
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[1].args,
            args(&[
                "-C",
                r"C:\dev\a",
                "symbolic-ref",
                "--quiet",
                "--short",
                "HEAD"
            ])
        );
    }

    #[test]
    fn default_branch_is_none_on_a_detached_head_without_remote() {
        let (git, _) = git(vec![Err(failed()), Err(failed())]);
        assert_eq!(git.default_branch(&local(r"C:\dev\a")).unwrap(), None);
    }

    #[test]
    fn default_branch_reports_spawn_failures() {
        let spawn = GitError::Spawn {
            program: "git".to_owned(),
            message: "not found".to_owned(),
        };
        let (git, _) = git(vec![Err(spawn.clone())]);
        assert_eq!(git.default_branch(&local(r"C:\dev\a")), Err(spawn));
    }

    #[test]
    fn process_runner_reports_stderr_of_failed_commands() {
        let invocation = Invocation {
            program: "git",
            args: args(&["no-such-command-for-toodo"]),
        };
        match ProcessRunner.run(&invocation) {
            Err(GitError::Failed { command, stderr }) => {
                assert_eq!(command, "git no-such-command-for-toodo");
                assert!(!stderr.is_empty());
            }
            other => panic!("expected a failed command, got {other:?}"),
        }
    }
}
