//! Drives herdr through its CLI. Commands print the socket API's `{"id", "result"}`
//! envelope as JSON on stdout; failures print `{"error": {"code", "message"}}` on stderr.
//! Commands for a workspace on an SSH host run against the herdr server of the saved herdr
//! machine whose SSH target is that host, through `herdr --machine <profile-id>`. herdr only
//! knows machines saved with `herdr machine add`, which needs an interactive terminal, so
//! the app never adds them itself.
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
    /// No saved herdr machine has the SSH host as its target.
    MachineNotSaved { host: String },
    /// Every saved herdr machine for the SSH host is disabled.
    MachineDisabled { host: String, profile_id: String },
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
            Self::MachineNotSaved { host } => write!(
                f,
                "SSH host '{host}' is not a saved herdr machine; run `herdr machine add {host}` \
                 in a terminal, then try again"
            ),
            Self::MachineDisabled { host, profile_id } => write!(
                f,
                "the herdr machine for SSH host '{host}' is disabled; run \
                 `herdr machine enable {profile_id}`, then try again"
            ),
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
        let invocation = invocation(None, &["status", "--json"]);
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
        let profile_id = self.profile_id(machine)?;
        let invocation = invocation(
            profile_id.as_deref(),
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
        let profile_id = self.profile_id(machine)?;
        let invocation = invocation(profile_id.as_deref(), &["workspace", "focus", workspace_id]);
        self.runner.run(&invocation)?;
        Ok(())
    }

    /// ID of the saved herdr machine to target for `machine`, or `None` for this machine.
    /// Among enabled profiles whose SSH target is the host, the one on herdr's default
    /// session wins.
    fn profile_id(&self, machine: &Machine) -> Result<Option<String>, HerdrError> {
        let Machine::Ssh(host) = machine else {
            return Ok(None);
        };
        let invocation = invocation(None, &["machine", "list", "--json"]);
        let output = self.runner.run(&invocation)?;
        let list = parse(&invocation, &output)?;
        let profiles = list
            .as_array()
            .ok_or_else(|| unexpected(&invocation, &output))?
            .iter()
            .map(SavedMachine::parse)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| unexpected(&invocation, &output))?;
        let matching: Vec<_> = profiles
            .iter()
            .filter(|profile| profile.target == host.as_str())
            .collect();
        let mut enabled = matching.iter().filter(|profile| profile.enabled);
        let chosen = enabled
            .clone()
            .find(|profile| profile.session == DEFAULT_SESSION)
            .or_else(|| enabled.next());
        match (chosen, matching.first()) {
            (Some(profile), _) => Ok(Some(profile.id.to_owned())),
            (None, Some(disabled)) => Err(HerdrError::MachineDisabled {
                host: host.as_str().to_owned(),
                profile_id: disabled.id.to_owned(),
            }),
            (None, None) => Err(HerdrError::MachineNotSaved {
                host: host.as_str().to_owned(),
            }),
        }
    }
}

/// Session herdr uses for a saved machine unless `--remote-session` names another.
const DEFAULT_SESSION: &str = "default";

/// One row of `herdr machine list --json`.
struct SavedMachine<'a> {
    id: &'a str,
    target: &'a str,
    session: &'a str,
    enabled: bool,
}

impl<'a> SavedMachine<'a> {
    fn parse(row: &'a Value) -> Option<Self> {
        Some(Self {
            id: row.get("id")?.as_str()?,
            target: row.get("target")?.as_str()?,
            session: row.get("session")?.as_str()?,
            enabled: row.get("enabled")?.as_bool()?,
        })
    }
}

/// `herdr [--machine <profile-id>] <args>`.
fn invocation(profile_id: Option<&str>, args: &[&str]) -> Invocation {
    let target = match profile_id {
        None => Vec::new(),
        Some(id) => vec!["--machine".to_owned(), id.to_owned()],
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

    fn devbox() -> Machine {
        Machine::Ssh(MachineLabel::parse("devbox").unwrap())
    }

    fn saved(id: &str, target: &str, session: &str, enabled: bool) -> String {
        format!(
            r#"{{"id":"{id}","label":"{target}","target":"{target}","session":"{session}",
                "enabled":{enabled},"selected":false}}"#
        )
    }

    fn machine_list(rows: &[String]) -> Result<String, GitError> {
        Ok(format!("[{}]", rows.join(",")))
    }

    #[test]
    fn ssh_machines_are_targeted_by_their_saved_profile_id() {
        let (herdr, runner) = herdr(vec![
            machine_list(&[
                saved("m1", "other", "default", true),
                saved("m2", "devbox", "default", true),
            ]),
            Ok(String::new()),
        ]);
        herdr.focus_workspace(&devbox(), "w3").unwrap();
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(
            invocations[0].args,
            ["machine", "list", "--json"].map(str::to_owned)
        );
        assert_eq!(
            invocations[1].args,
            ["--machine", "m2", "workspace", "focus", "w3"].map(str::to_owned)
        );
    }

    #[test]
    fn enabled_profiles_on_the_default_session_are_preferred() {
        let (herdr, runner) = herdr(vec![
            machine_list(&[
                saved("m1", "devbox", "default", false),
                saved("m2", "devbox", "agents", true),
                saved("m3", "devbox", "default", true),
            ]),
            Ok(String::new()),
            machine_list(&[
                saved("m1", "devbox", "default", false),
                saved("m2", "devbox", "agents", true),
            ]),
            Ok(String::new()),
        ]);
        herdr.focus_workspace(&devbox(), "w3").unwrap();
        herdr.focus_workspace(&devbox(), "w3").unwrap();
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(invocations[1].args[1], "m3");
        assert_eq!(invocations[3].args[1], "m2");
    }

    #[test]
    fn hosts_without_an_enabled_saved_machine_are_reported() {
        let (herdr, runner) = herdr(vec![
            machine_list(&[]),
            machine_list(&[saved("m1", "devbox", "default", false)]),
        ]);
        assert_eq!(
            herdr.create_worktree(&devbox(), &WORKTREE),
            Err(HerdrError::MachineNotSaved {
                host: "devbox".to_owned()
            })
        );
        assert_eq!(
            herdr.create_worktree(&devbox(), &WORKTREE),
            Err(HerdrError::MachineDisabled {
                host: "devbox".to_owned(),
                profile_id: "m1".to_owned()
            })
        );
        assert_eq!(runner.invocations.lock().unwrap().len(), 2);
    }

    #[test]
    fn malformed_machine_lists_are_unexpected() {
        let (herdr, _) = herdr(vec![Ok(r#"[{"id":"m1"}]"#.to_owned())]);
        assert!(matches!(
            herdr.create_worktree(&devbox(), &WORKTREE),
            Err(HerdrError::UnexpectedOutput { .. })
        ));
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
