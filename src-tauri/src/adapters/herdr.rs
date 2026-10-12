//! Drives herdr through its CLI. Commands print the socket API's `{"id", "result"}`
//! envelope as JSON on stdout; failures print `{"error": {"code", "message"}}` on stderr.
//! Commands for a workspace on a saved SSH machine run against that machine's herdr
//! server through `herdr --machine <label>`.
//!
//! Processes are started through the [`Runner`] of the git adapter, so tests can fake
//! them the same way.

use std::fmt;

use serde_json::Value;

use super::git::{GitError, Invocation, Runner, describe};
use crate::domain::model::Machine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HerdrError {
    /// herdr could not be started, usually because it is not installed or not on `PATH`.
    Spawn { message: String },
    /// herdr exited with a non-zero status. `code` is the API error code when herdr
    /// reported one.
    Failed {
        command: String,
        code: Option<String>,
        message: String,
    },
    /// herdr succeeded but printed something the app cannot use.
    UnexpectedOutput { command: String, output: String },
}

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { message } => write!(
                f,
                "failed to run herdr (is it installed and on PATH?): {message}"
            ),
            Self::Failed {
                command, message, ..
            } => write!(f, "`{command}` failed: {message}"),
            Self::UnexpectedOutput { command, output } => {
                write!(f, "`{command}` printed unexpected output: {output:?}")
            }
        }
    }
}

impl std::error::Error for HerdrError {}

impl From<GitError> for HerdrError {
    fn from(error: GitError) -> Self {
        match error {
            GitError::Spawn { message, .. } => Self::Spawn { message },
            GitError::Failed { command, stderr } => {
                let reported = serde_json::from_str::<Value>(&stderr)
                    .ok()
                    .and_then(|value| {
                        let error = value.get("error")?;
                        let message = error.get("message")?.as_str()?.to_owned();
                        let code = error.get("code").and_then(Value::as_str).map(str::to_owned);
                        Some((code, message))
                    });
                let (code, message) = reported.unwrap_or((None, stderr));
                Self::Failed {
                    command,
                    code,
                    message,
                }
            }
            GitError::UnexpectedOutput { command, output } => {
                Self::UnexpectedOutput { command, output }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub running: bool,
    pub version: Option<String>,
}

/// A git worktree to create and open as a herdr workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewWorktree<'a> {
    pub repository: &'a str,
    pub path: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
}

pub struct Herdr {
    runner: Box<dyn Runner>,
}

impl Herdr {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner }
    }

    /// Status of the herdr server on this machine. A stopped server is reported as
    /// `running: false`, not as an error.
    pub fn server_status(&self) -> Result<ServerStatus, HerdrError> {
        let invocation = invocation(&Machine::Local, &["status", "--json"]);
        let output = self.runner.run(&invocation)?;
        let server = parse(&invocation, &output)?.get("server").cloned();
        let running = server
            .as_ref()
            .and_then(|server| server.get("running"))
            .and_then(Value::as_bool)
            .ok_or_else(|| unexpected(&invocation, &output))?;
        let version = server
            .as_ref()
            .and_then(|server| server.get("version"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(ServerStatus { running, version })
    }

    /// Creates the worktree on `machine` and opens it as a herdr workspace without
    /// focusing it. Returns the ID of the herdr workspace.
    pub fn create_worktree(
        &self,
        machine: &Machine,
        worktree: &NewWorktree<'_>,
    ) -> Result<String, HerdrError> {
        let invocation = invocation(
            machine,
            &[
                "worktree",
                "create",
                "--cwd",
                worktree.repository,
                "--path",
                worktree.path,
                "--branch",
                worktree.branch,
                "--base",
                worktree.base,
                "--no-focus",
            ],
        );
        let output = self.runner.run(&invocation)?;
        parse(&invocation, &output)?
            .pointer("/result/workspace/workspace_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| unexpected(&invocation, &output))
    }

    pub fn focus_workspace(&self, machine: &Machine, workspace_id: &str) -> Result<(), HerdrError> {
        let invocation = invocation(machine, &["workspace", "focus", workspace_id]);
        self.runner.run(&invocation)?;
        Ok(())
    }
}

fn invocation(machine: &Machine, args: &[&str]) -> Invocation {
    let target = match machine {
        Machine::Local => Vec::new(),
        Machine::Ssh(label) => vec!["--machine".to_owned(), label.as_str().to_owned()],
    };
    Invocation {
        program: "herdr",
        args: target
            .into_iter()
            .chain(args.iter().map(|arg| (*arg).to_owned()))
            .collect(),
    }
}

fn parse(invocation: &Invocation, output: &str) -> Result<Value, HerdrError> {
    serde_json::from_str(output).map_err(|_| unexpected(invocation, output))
}

fn unexpected(invocation: &Invocation, output: &str) -> HerdrError {
    HerdrError::UnexpectedOutput {
        command: describe(invocation),
        output: output.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::adapters::git::fake::FakeRunner;
    use crate::domain::model::MachineLabel;

    fn herdr(results: Vec<Result<String, GitError>>) -> (Herdr, Arc<FakeRunner>) {
        let runner = Arc::new(FakeRunner::new(results));
        (Herdr::new(Box::new(runner.clone())), runner)
    }

    const WORKTREE: NewWorktree<'static> = NewWorktree {
        repository: r"C:\dev\toodo",
        path: r"D:\wt\toodo\feat-add-x-3",
        branch: "feat/add-x-3",
        base: "main",
    };

    const CREATED: &str = r#"{"id":"cli:worktree:create","result":{"type":"worktree_created",
        "workspace":{"workspace_id":"w27","label":"feat-add-x-3"},
        "worktree":{"branch":"feat/add-x-3","path":"D:\\wt\\toodo\\feat-add-x-3"}}}"#;

    fn failed(stderr: &str) -> GitError {
        GitError::Failed {
            command: "herdr worktree create".to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    #[test]
    fn create_worktree_returns_the_herdr_workspace() {
        let (herdr, runner) = herdr(vec![Ok(CREATED.to_owned())]);
        assert_eq!(
            herdr.create_worktree(&Machine::Local, &WORKTREE).unwrap(),
            "w27"
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            Invocation {
                program: "herdr",
                args: [
                    "worktree",
                    "create",
                    "--cwd",
                    r"C:\dev\toodo",
                    "--path",
                    r"D:\wt\toodo\feat-add-x-3",
                    "--branch",
                    "feat/add-x-3",
                    "--base",
                    "main",
                    "--no-focus",
                ]
                .map(str::to_owned)
                .to_vec(),
            }
        );
    }

    #[test]
    fn ssh_machines_are_targeted_with_the_machine_flag() {
        let (herdr, runner) = herdr(vec![Ok(String::new())]);
        let machine = Machine::Ssh(MachineLabel::parse("devbox").unwrap());
        herdr.focus_workspace(&machine, "w3").unwrap();
        assert_eq!(
            runner.invocations.lock().unwrap()[0].args,
            ["--machine", "devbox", "workspace", "focus", "w3"].map(str::to_owned)
        );
    }

    #[test]
    fn api_errors_on_stderr_are_parsed() {
        let stderr = r#"{"error":{"code":"worktree_create_failed","message":"fatal: invalid reference: nope"},"id":"cli:worktree:create"}"#;
        let (herdr, _) = herdr(vec![Err(failed(stderr))]);
        assert_eq!(
            herdr.create_worktree(&Machine::Local, &WORKTREE),
            Err(HerdrError::Failed {
                command: "herdr worktree create".to_owned(),
                code: Some("worktree_create_failed".to_owned()),
                message: "fatal: invalid reference: nope".to_owned(),
            })
        );
    }

    #[test]
    fn plain_text_errors_are_kept() {
        let stderr = "error: unknown machine 'devbox'; use `herdr machine list`";
        let (herdr, _) = herdr(vec![Err(failed(stderr))]);
        let error = herdr
            .create_worktree(&Machine::Local, &WORKTREE)
            .unwrap_err();
        assert!(matches!(
            &error,
            HerdrError::Failed { code: None, message, .. } if message == stderr
        ));
    }

    #[test]
    fn create_output_without_a_workspace_is_unexpected() {
        let (herdr, _) = herdr(vec![Ok(r#"{"id":"x","result":{}}"#.to_owned())]);
        assert!(matches!(
            herdr.create_worktree(&Machine::Local, &WORKTREE),
            Err(HerdrError::UnexpectedOutput { .. })
        ));
    }

    #[test]
    fn server_status_reports_a_stopped_server() {
        let (herdr, runner) = herdr(vec![
            Ok(r#"{"server":{"running":true,"version":"0.9.3"}}"#.to_owned()),
            Ok(r#"{"server":{"running":false,"version":null}}"#.to_owned()),
            Ok("not json".to_owned()),
        ]);
        assert_eq!(
            herdr.server_status().unwrap(),
            ServerStatus {
                running: true,
                version: Some("0.9.3".to_owned())
            }
        );
        assert_eq!(
            herdr.server_status().unwrap(),
            ServerStatus {
                running: false,
                version: None
            }
        );
        assert!(matches!(
            herdr.server_status(),
            Err(HerdrError::UnexpectedOutput { .. })
        ));
        assert_eq!(
            runner.invocations.lock().unwrap()[0].args,
            ["status", "--json"].map(str::to_owned)
        );
    }

    #[test]
    fn missing_herdr_is_a_spawn_error() {
        let (herdr, _) = herdr(vec![Err(GitError::Spawn {
            program: "herdr".to_owned(),
            message: "program not found".to_owned(),
        })]);
        assert_eq!(
            herdr.server_status(),
            Err(HerdrError::Spawn {
                message: "program not found".to_owned()
            })
        );
    }
}
