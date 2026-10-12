//! Runs git for a workspace: `git.exe` for local workspaces and `ssh.exe <machine> git ...`
//! for workspaces on a saved herdr SSH machine.

use std::fmt;
use std::process::{Command, Stdio};

use crate::domain::model::{MachineLabel, WorkspaceLocation};
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

    /// Runs an invocation in a terminal window the user can see and answer, and waits for
    /// it to exit. Every argument must be a [plain word](is_plain_word).
    fn run_in_terminal(&self, invocation: &Invocation) -> Result<(), GitError>;
}

/// Whether `word` means the same to `cmd.exe` and every shell without quoting.
pub fn is_plain_word(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | ':' | '/' | '=')
        })
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

    fn run_in_terminal(&self, invocation: &Invocation) -> Result<(), GitError> {
        let spawn_error = |message: String| GitError::Spawn {
            program: invocation.program.to_owned(),
            message,
        };
        if let Some(arg) = invocation.args.iter().find(|arg| !is_plain_word(arg)) {
            return Err(spawn_error(format!(
                "argument {arg:?} cannot be passed to a terminal"
            )));
        }
        // A new console window; on failure it waits for a key so the user can read the
        // error before it closes.
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            let mut command = Command::new("cmd.exe");
            command
                .raw_arg(format!("/c {} || (pause & exit 1)", describe(invocation)))
                .creation_flags(CREATE_NEW_CONSOLE);
            command
        };
        // Elsewhere the app's own terminal, which is only used during development.
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new(invocation.program);
            command.args(&invocation.args);
            command
        };
        let status = command
            .status()
            .map_err(|error| spawn_error(error.to_string()))?;
        if !status.success() {
            return Err(GitError::Failed {
                command: describe(invocation),
                stderr: format!("exited with {status}"),
            });
        }
        Ok(())
    }
}

pub(super) fn describe(invocation: &Invocation) -> String {
    std::iter::once(invocation.program)
        .chain(invocation.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes one word for the POSIX shell that runs an SSH remote command.
pub(super) fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `ssh.exe` running `remote` on `machine` without prompting for passwords or host keys.
pub(super) fn ssh_invocation(machine: &MachineLabel, remote: String) -> Invocation {
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

    /// Gives an empty repository, one without any commit, its first commit on `branch` so
    /// worktrees can branch off it, and pushes it to `origin` when that remote exists.
    /// The commit is empty: files the user staged stay staged. Returns whether it
    /// committed; a repository that already has commits is left alone.
    pub fn create_initial_commit(
        &self,
        repo: &WorkspaceLocation,
        branch: &str,
    ) -> Result<bool, GitError> {
        let path = Some(location_path(repo));
        let local_branch = format!("refs/heads/{branch}");
        for revision in [local_branch.as_str(), "HEAD"] {
            match self.run(repo, path, &["rev-parse", "--verify", "--quiet", revision]) {
                Ok(_) => return Ok(false),
                Err(GitError::Failed { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let has_origin = self
            .run(repo, path, &["remote"])?
            .lines()
            .any(|remote| remote.trim() == "origin");
        self.run(repo, path, &["symbolic-ref", "HEAD", &local_branch])?;
        self.run(
            repo,
            path,
            &["commit", "--allow-empty", "--only", "-m", "Initial commit"],
        )?;
        if has_origin {
            self.run(repo, path, &["push", "--set-upstream", "origin", branch])?;
        }
        Ok(true)
    }

    /// Commits reachable from `branch` but not from `base`, oldest first, read in the
    /// repository or worktree at `dir` on the machine of `location`.
    pub fn commits_between(
        &self,
        location: &WorkspaceLocation,
        dir: &str,
        base: &str,
        branch: &str,
    ) -> Result<Vec<LoggedCommit>, GitError> {
        let range = format!("{base}..{branch}");
        let args = ["log", "--reverse", LOG_FORMAT, &range, "--"];
        let output = self.run(location, Some(dir), &args)?;
        output
            .split(RECORD_SEPARATOR)
            .map(|record| record.trim_start_matches(['\r', '\n']))
            .filter(|record| !record.trim().is_empty())
            .map(|record| {
                parse_commit(record).ok_or_else(|| GitError::UnexpectedOutput {
                    command: format!("git log {range}"),
                    output: record.to_owned(),
                })
            })
            .collect()
    }

    /// Pushes `branch` to `origin` and makes it the upstream of the local branch.
    pub fn push_branch(
        &self,
        location: &WorkspaceLocation,
        dir: &str,
        branch: &str,
    ) -> Result<(), GitError> {
        self.run(
            location,
            Some(dir),
            &["push", "--set-upstream", "origin", branch],
        )?;
        Ok(())
    }

    /// URL of the `origin` remote of the repository at `dir`.
    pub fn origin_url(&self, location: &WorkspaceLocation, dir: &str) -> Result<String, GitError> {
        let output = self.run(location, Some(dir), &["remote", "get-url", "origin"])?;
        let url = output.trim();
        if url.is_empty() {
            return Err(GitError::UnexpectedOutput {
                command: "git remote get-url origin".to_owned(),
                output: output.clone(),
            });
        }
        Ok(url.to_owned())
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
                ssh_invocation(machine, remote)
            }
        };
        self.runner.run(&invocation)
    }
}

/// A commit as printed by [`Git::commits_between`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedCommit {
    pub sha: String,
    /// Subject and body.
    pub message: String,
    /// `Name <email>` of the author.
    pub author: String,
    /// Committer timestamp in seconds since the Unix epoch.
    pub committed_at: i64,
}

// Fields are separated by US (0x1f) and commits by RS (0x1e), which cannot appear in
// the fields themselves.
const LOG_FORMAT: &str = "--format=%H%x1f%an <%ae>%x1f%ct%x1f%B%x1e";
const FIELD_SEPARATOR: char = '\u{1f}';
const RECORD_SEPARATOR: char = '\u{1e}';

fn parse_commit(record: &str) -> Option<LoggedCommit> {
    let mut fields = record.splitn(4, FIELD_SEPARATOR);
    let sha = fields.next()?.trim();
    let author = fields.next()?;
    let committed_at = fields.next()?.trim().parse().ok()?;
    let message = fields.next()?.trim_end();
    if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(LoggedCommit {
        sha: sha.to_owned(),
        message: message.to_owned(),
        author: author.to_owned(),
        committed_at,
    })
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

        /// Takes the next scripted result like [`Runner::run`] and drops its output.
        fn run_in_terminal(&self, invocation: &Invocation) -> Result<(), GitError> {
            self.run(invocation).map(drop)
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
    fn an_empty_repository_gets_an_initial_commit_pushed_to_origin() {
        let (git, runner) = git(vec![
            Err(failed()),
            Err(failed()),
            Ok("origin\n".to_owned()),
            Ok(String::new()),
            Ok(String::new()),
            Ok(String::new()),
        ]);
        assert!(
            git.create_initial_commit(&local(r"C:\dev\a"), "main")
                .unwrap()
        );
        let commands: Vec<Vec<String>> = runner
            .invocations
            .lock()
            .unwrap()
            .iter()
            .map(|invocation| invocation.args[2..].to_vec())
            .collect();
        assert_eq!(
            commands,
            [
                args(&["rev-parse", "--verify", "--quiet", "refs/heads/main"]),
                args(&["rev-parse", "--verify", "--quiet", "HEAD"]),
                args(&["remote"]),
                args(&["symbolic-ref", "HEAD", "refs/heads/main"]),
                args(&["commit", "--allow-empty", "--only", "-m", "Initial commit"]),
                args(&["push", "--set-upstream", "origin", "main"]),
            ]
        );
    }

    #[test]
    fn an_empty_repository_without_origin_is_only_committed_to() {
        let (git, runner) = git(vec![
            Err(failed()),
            Err(failed()),
            Ok("upstream\n".to_owned()),
            Ok(String::new()),
            Ok(String::new()),
        ]);
        assert!(
            git.create_initial_commit(&remote("/srv/a"), "main")
                .unwrap()
        );
        assert_eq!(runner.invocations.lock().unwrap().len(), 5);
    }

    #[test]
    fn a_repository_with_commits_is_left_alone() {
        let (git, runner) = git(vec![
            Ok("abc\n".to_owned()),
            Err(failed()),
            Ok("def\n".to_owned()),
        ]);
        let repo = local(r"C:\dev\a");
        assert!(!git.create_initial_commit(&repo, "main").unwrap());
        assert!(!git.create_initial_commit(&repo, "develop").unwrap());
        assert_eq!(runner.invocations.lock().unwrap().len(), 3);
    }

    const SHA_A: &str = "1111111111111111111111111111111111111111";
    const SHA_B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn commits_between_parses_log_records_oldest_first() {
        let log = format!(
            "{SHA_A}\u{1f}Ada <ada@example.com>\u{1f}1700000000\u{1f}Add login\n\nWith a body\n\u{1e}\n\
             {SHA_B}\u{1f}Bob <bob@example.com>\u{1f}1700000100\u{1f}Fix login\n\u{1e}\n"
        );
        let (git, runner) = git(vec![Ok(log)]);
        let commits = git
            .commits_between(&local(r"C:\dev\a"), r"D:\wt\a\feat-x-1", "main", "feat/x-1")
            .unwrap();
        assert_eq!(
            commits,
            vec![
                LoggedCommit {
                    sha: SHA_A.to_owned(),
                    message: "Add login\n\nWith a body".to_owned(),
                    author: "Ada <ada@example.com>".to_owned(),
                    committed_at: 1_700_000_000,
                },
                LoggedCommit {
                    sha: SHA_B.to_owned(),
                    message: "Fix login".to_owned(),
                    author: "Bob <bob@example.com>".to_owned(),
                    committed_at: 1_700_000_100,
                },
            ]
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[0].args,
            args(&[
                "-C",
                r"D:\wt\a\feat-x-1",
                "log",
                "--reverse",
                "--format=%H%x1f%an <%ae>%x1f%ct%x1f%B%x1e",
                "main..feat/x-1",
                "--",
            ])
        );
    }

    #[test]
    fn commits_between_is_empty_without_new_commits_and_rejects_garbage() {
        let (git, _) = git(vec![Ok(String::new()), Ok("not a log\n".to_owned())]);
        let location = local(r"C:\dev\a");
        assert_eq!(
            git.commits_between(&location, r"C:\dev\a", "main", "feat/x-1")
                .unwrap(),
            Vec::new()
        );
        assert!(matches!(
            git.commits_between(&location, r"C:\dev\a", "main", "feat/x-1"),
            Err(GitError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn push_and_origin_url_run_in_the_given_directory_on_the_machine() {
        let (git, runner) = git(vec![
            Ok(String::new()),
            Ok("git@github.com:acme/a.git\n".to_owned()),
        ]);
        git.push_branch(&remote("/srv/a"), "/wt/a/feat-x-1", "feat/x-1")
            .unwrap();
        assert_eq!(
            git.origin_url(&remote("/srv/a"), "/wt/a/feat-x-1").unwrap(),
            "git@github.com:acme/a.git"
        );
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(
            invocations[0].args[4],
            "git '-C' '/wt/a/feat-x-1' 'push' '--set-upstream' 'origin' 'feat/x-1'"
        );
        assert_eq!(
            invocations[1].args[4],
            "git '-C' '/wt/a/feat-x-1' 'remote' 'get-url' 'origin'"
        );
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
