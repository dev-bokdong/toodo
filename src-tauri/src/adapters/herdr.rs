//! Drives herdr through its CLI. Commands print the socket API's `{"id", "result"}`
//! envelope as JSON on stdout; failures print `{"error": {"code", "message"}}` on stderr.
//! Commands for a workspace on an SSH host run against the herdr server of the saved herdr
//! machine whose SSH target is that host, through `herdr --machine <profile-id>`. herdr only
//! knows machines saved with `herdr machine add`, which may ask to install or replace the
//! remote herdr, so the app runs it in a terminal window the user answers and never
//! installs herdr itself.
//!
//! Processes are started through the [`Runner`] of the git adapter, so tests can fake
//! them the same way.

use std::fmt;

use serde_json::Value;

use super::git::{GitError, Invocation, Runner, describe, is_plain_word};
use crate::domain::model::{Machine, MachineLabel};

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
    /// The SSH host cannot be passed to `herdr machine add` safely.
    InvalidHost { host: String },
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
            Self::InvalidHost { host } => write!(
                f,
                "SSH host {host:?} must use only letters, digits, and . _ - @ : / = \
                 and must not start with '-'"
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
        workspace_id(&invocation, &output)
    }

    /// Opens the existing worktree at `path` of `repository` as a herdr workspace without
    /// focusing it, or finds the workspace that already has it open. Returns its ID.
    pub fn open_worktree(
        &self,
        machine: &Machine,
        repository: &str,
        path: &str,
    ) -> Result<String, HerdrError> {
        let profile_id = self.profile_id(machine)?;
        let invocation = invocation(
            profile_id.as_deref(),
            &[
                "worktree",
                "open",
                "--cwd",
                repository,
                "--path",
                path,
                "--no-focus",
            ],
        );
        let output = self.runner.run(&invocation)?;
        workspace_id(&invocation, &output)
    }

    /// Removes the worktree checkout of a herdr workspace and closes the workspace; the
    /// branch is kept. Without `force`, herdr refuses a checkout with modified or
    /// untracked files.
    pub fn remove_worktree(
        &self,
        machine: &Machine,
        workspace_id: &str,
        force: bool,
    ) -> Result<(), HerdrError> {
        let profile_id = self.profile_id(machine)?;
        let mut args = vec!["worktree", "remove", "--workspace", workspace_id];
        if force {
            args.push("--force");
        }
        let invocation = invocation(profile_id.as_deref(), &args);
        let output = self.runner.run(&invocation)?;
        match parse(&invocation, &output)?
            .pointer("/result/type")
            .and_then(Value::as_str)
        {
            Some("worktree_removed") => Ok(()),
            _ => Err(unexpected(&invocation, &output)),
        }
    }

    /// Closes a herdr workspace. The worktree checkout it was bound to stays on disk.
    pub fn close_workspace(&self, machine: &Machine, workspace_id: &str) -> Result<(), HerdrError> {
        let profile_id = self.profile_id(machine)?;
        let invocation = invocation(profile_id.as_deref(), &["workspace", "close", workspace_id]);
        self.runner.run(&invocation)?;
        Ok(())
    }

    pub fn focus_workspace(&self, machine: &Machine, workspace_id: &str) -> Result<(), HerdrError> {
        let profile_id = self.profile_id(machine)?;
        let invocation = invocation(profile_id.as_deref(), &["workspace", "focus", workspace_id]);
        self.runner.run(&invocation)?;
        Ok(())
    }

    /// Machines saved in herdr, in herdr's order.
    pub fn list_machines(&self) -> Result<Vec<SavedMachine>, HerdrError> {
        let invocation = invocation(None, &["machine", "list", "--json"]);
        let output = self.runner.run(&invocation)?;
        parse(&invocation, &output)?
            .as_array()
            .and_then(|rows| rows.iter().map(SavedMachine::parse).collect())
            .ok_or_else(|| unexpected(&invocation, &output))
    }

    /// Runs `herdr machine add <host>` in a terminal window and waits for the user to
    /// finish it. herdr asks there before installing or replacing the remote herdr, and
    /// saves nothing when the user cancels.
    pub fn add_machine(&self, host: &MachineLabel) -> Result<(), HerdrError> {
        if !is_plain_word(host.as_str()) || host.as_str().starts_with('-') {
            return Err(HerdrError::InvalidHost {
                host: host.as_str().to_owned(),
            });
        }
        let invocation = invocation(None, &["machine", "add", host.as_str()]);
        Ok(self.runner.run_in_terminal(&invocation)?)
    }

    /// ID of the saved herdr machine to target for `machine`, or `None` for this machine.
    /// Among enabled profiles whose SSH target is the host, the one on herdr's default
    /// session wins.
    fn profile_id(&self, machine: &Machine) -> Result<Option<String>, HerdrError> {
        let Machine::Ssh(host) = machine else {
            return Ok(None);
        };
        let profiles = self.list_machines()?;
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
            (Some(profile), _) => Ok(Some(profile.id.clone())),
            (None, Some(disabled)) => Err(HerdrError::MachineDisabled {
                host: host.as_str().to_owned(),
                profile_id: disabled.id.clone(),
            }),
            (None, None) => Err(HerdrError::MachineNotSaved {
                host: host.as_str().to_owned(),
            }),
        }
    }
}

/// Session herdr uses for a saved machine unless `--remote-session` names another.
const DEFAULT_SESSION: &str = "default";

/// A machine saved with `herdr machine add`: one row of `herdr machine list --json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedMachine {
    /// Opaque profile ID accepted by `herdr --machine`.
    pub id: String,
    /// Name shown in the herdr sidebar.
    pub label: String,
    /// SSH target, usually a host alias from the SSH config.
    pub target: String,
    /// herdr session on the remote machine.
    pub session: String,
    pub enabled: bool,
}

impl SavedMachine {
    fn parse(row: &Value) -> Option<Self> {
        let text = |key: &str| row.get(key)?.as_str().map(str::to_owned);
        Some(Self {
            id: text("id")?,
            label: text("label")?,
            target: text("target")?,
            session: text("session")?,
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

/// The workspace ID in the output of `herdr worktree create` or `herdr worktree open`.
fn workspace_id(invocation: &Invocation, output: &str) -> Result<String, HerdrError> {
    parse(invocation, output)?
        .pointer("/result/workspace/workspace_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| unexpected(invocation, output))
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
    fn saved_machines_are_listed_in_order() {
        let (herdr, _) = herdr(vec![machine_list(&[
            saved("m2", "devbox", "default", true),
            saved("m1", "gpu", "agents", false),
        ])]);
        assert_eq!(
            herdr.list_machines().unwrap(),
            [
                SavedMachine {
                    id: "m2".to_owned(),
                    label: "devbox".to_owned(),
                    target: "devbox".to_owned(),
                    session: "default".to_owned(),
                    enabled: true,
                },
                SavedMachine {
                    id: "m1".to_owned(),
                    label: "gpu".to_owned(),
                    target: "gpu".to_owned(),
                    session: "agents".to_owned(),
                    enabled: false,
                },
            ]
        );
    }

    #[test]
    fn machines_are_added_in_a_terminal() {
        let (herdr, runner) = herdr(vec![Ok(String::new())]);
        herdr
            .add_machine(&MachineLabel::parse("admin@analysis-server").unwrap())
            .unwrap();
        assert_eq!(
            runner.invocations.lock().unwrap()[0],
            Invocation {
                program: "herdr",
                args: ["machine", "add", "admin@analysis-server"]
                    .map(str::to_owned)
                    .to_vec(),
            }
        );
    }

    #[test]
    fn hosts_that_need_quoting_are_not_added() {
        let (herdr, runner) = herdr(Vec::new());
        for host in ["dev box", "devbox&calc", "-oProxyCommand=x", "dev\"box"] {
            assert_eq!(
                herdr.add_machine(&MachineLabel::parse(host).unwrap()),
                Err(HerdrError::InvalidHost {
                    host: host.to_owned()
                }),
                "{host}"
            );
        }
        assert!(runner.invocations.lock().unwrap().is_empty());
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
    fn remove_worktree_removes_the_workspace_checkout() {
        let removed = r#"{"id":"cli:worktree:remove","result":{"forced":true,
            "path":"D:\\wt\\toodo\\feat-add-x-3","type":"worktree_removed","workspace_id":"w27"}}"#;
        let (herdr, runner) = herdr(vec![
            Ok(removed.to_owned()),
            Ok(removed.to_owned()),
            Ok(r#"{"id":"x","result":{"type":"ok"}}"#.to_owned()),
        ]);
        herdr
            .remove_worktree(&Machine::Local, "w27", false)
            .unwrap();
        herdr.remove_worktree(&Machine::Local, "w27", true).unwrap();
        assert!(matches!(
            herdr.remove_worktree(&Machine::Local, "w27", false),
            Err(HerdrError::UnexpectedOutput { .. })
        ));
        let invocations = runner.invocations.lock().unwrap();
        assert_eq!(
            invocations[0].args,
            ["worktree", "remove", "--workspace", "w27"].map(str::to_owned)
        );
        assert_eq!(
            invocations[1].args,
            ["worktree", "remove", "--workspace", "w27", "--force"].map(str::to_owned)
        );
    }

    #[test]
    fn close_workspace_closes_it_on_the_machine() {
        let (herdr, runner) = herdr(vec![
            machine_list(&[saved("m2", "devbox", "default", true)]),
            Ok(r#"{"id":"cli:workspace:close","result":{"type":"ok"}}"#.to_owned()),
        ]);
        herdr.close_workspace(&devbox(), "w27").unwrap();
        assert_eq!(
            runner.invocations.lock().unwrap()[1].args,
            ["--machine", "m2", "workspace", "close", "w27"].map(str::to_owned)
        );
    }

    #[test]
    fn open_worktree_returns_the_herdr_workspace() {
        let opened = r#"{"id":"cli:worktree:open","result":{"already_open":false,
            "type":"worktree_opened","workspace":{"workspace_id":"w30"}}}"#;
        let (herdr, runner) = herdr(vec![
            machine_list(&[saved("m2", "devbox", "default", true)]),
            Ok(opened.to_owned()),
        ]);
        assert_eq!(
            herdr
                .open_worktree(&devbox(), "/srv/toodo", "/wt/toodo/feat-x-3")
                .unwrap(),
            "w30"
        );
        assert_eq!(
            runner.invocations.lock().unwrap()[1].args,
            [
                "--machine",
                "m2",
                "worktree",
                "open",
                "--cwd",
                "/srv/toodo",
                "--path",
                "/wt/toodo/feat-x-3",
                "--no-focus",
            ]
            .map(str::to_owned)
        );
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
