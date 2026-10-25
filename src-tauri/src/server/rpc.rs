//! `POST /api/rpc/<command>`: runs one of the commands the desktop app registers with
//! Tauri, with the same JSON arguments (camelCase keys, as Tauri passes them) and the same
//! JSON result, so the frontend's generated bindings work unchanged over HTTP.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::Services;
use crate::commands::events::Notifier;
use crate::commands::types::{
    BackupSettings, CriterionId, Machine, MachineSettings, MemoryContent, MemoryId, NewSshHost,
    NewTask, NewWorkspace, TaskEdit, TaskId, Timestamp, Workspace, WorkspaceId,
};
use crate::commands::{
    CommandError, CommandErrorKind, CommandResult, State, backup, completion, events, freshness,
    herdr, issues, lead, machines, mcp, memory, review, tasks, workspaces,
};

/// What running a command gave.
#[derive(Debug)]
pub enum Reply {
    /// The command's result, as JSON.
    Done(serde_json::Value),
    /// The command failed; the frontend gets the error as the desktop app reports it.
    Failed(CommandError),
    /// No command has this name.
    UnknownCommand,
    /// The arguments do not match the command's parameters.
    BadArguments(String),
}

impl Reply {
    pub(super) fn value(value: impl Serialize) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => Self::Done(value),
            Err(error) => Self::Failed(CommandError::internal(format!(
                "could not encode the result: {error}"
            ))),
        }
    }

    pub(super) fn result(result: CommandResult<impl Serialize>) -> Self {
        match result {
            Ok(value) => Self::value(value),
            Err(error) => Self::Failed(error),
        }
    }
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        match self {
            Self::Done(value) => json(StatusCode::OK, &value),
            Self::Failed(error) => {
                let status = match error.kind {
                    CommandErrorKind::InvalidInput => StatusCode::UNPROCESSABLE_ENTITY,
                    CommandErrorKind::NotFound => StatusCode::NOT_FOUND,
                    CommandErrorKind::Rejected => StatusCode::CONFLICT,
                    CommandErrorKind::External => StatusCode::BAD_GATEWAY,
                    CommandErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
                };
                json(status, &error)
            }
            Self::UnknownCommand => (StatusCode::NOT_FOUND, "unknown command").into_response(),
            Self::BadArguments(message) => (
                StatusCode::BAD_REQUEST,
                format!("invalid arguments: {message}"),
            )
                .into_response(),
        }
    }
}

pub(super) fn json(status: StatusCode, body: &impl Serialize) -> Response {
    match serde_json::to_vec(body) {
        Ok(body) => (status, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

pub async fn handle(
    axum::extract::State(services): axum::extract::State<Arc<Services>>,
    Path(command): Path<String>,
    body: Bytes,
) -> Reply {
    let args = if body.is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(args) => args,
            Err(error) => return Reply::BadArguments(error.to_string()),
        }
    };
    let reply = dispatch(services.clone(), &command, args).await;
    // Changes the user makes in the app are not news to them: notifications only report
    // what changed on its own (agents, pull requests, background checks).
    services.push.tasks_seen();
    reply
}

fn parse<T: DeserializeOwned>(args: serde_json::Value) -> Result<T, Reply> {
    let args = match args {
        serde_json::Value::Null => serde_json::Value::Object(serde_json::Map::new()),
        args => args,
    };
    serde_json::from_value(args).map_err(|error| Reply::BadArguments(error.to_string()))
}

/// Runs a command that blocks (git, gh, herdr, or the database) off the async threads.
async fn blocking(run: impl FnOnce() -> Reply + Send + 'static) -> Reply {
    tokio::task::spawn_blocking(run)
        .await
        .unwrap_or_else(|error| {
            Reply::Failed(CommandError::internal(format!("command panicked: {error}")))
        })
}

/// Declares the commands: each has a name, its parameters (the JSON arguments), and how it
/// runs, `sync` on a blocking thread or `async` on the runtime. `$s` names the services in
/// the calls. Also lists the names in `COMMANDS`.
macro_rules! commands {
    ($s:ident => { $( $name:literal ( $($field:ident : $ty:ty),* ) => $mode:ident $call:expr ; )* }) => {
        #[cfg(test)]
        pub const COMMANDS: &[&str] = &[$($name),*];

        pub async fn dispatch(
            services: Arc<Services>,
            command: &str,
            args: serde_json::Value,
        ) -> Reply {
            match command {
                $( $name => {
                    #[derive(serde::Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Args { $($field: $ty),* }
                    let Args { $($field),* } = match parse::<Args>(args) {
                        Ok(args) => args,
                        Err(reply) => return reply,
                    };
                    commands!(@$mode services, $s, $call)
                } )*
                _ => Reply::UnknownCommand,
            }
        }
    };
    (@sync $services:ident, $s:ident, $call:expr) => {
        blocking(move || {
            #[allow(unused_variables)]
            let $s: &Services = &$services;
            $call
        })
        .await
    };
    (@async $services:ident, $s:ident, $call:expr) => {{
        #[allow(unused_variables)]
        let $s: &Services = &$services;
        $call
    }};
}

commands!(s => {
    "app_info"() => sync Reply::value(crate::commands::app_info());

    "list_workspaces"() => sync Reply::result(workspaces::list_workspaces(State(&s.storage)));
    "get_workspace"(id: WorkspaceId) => sync Reply::result(workspaces::get_workspace(State(&s.storage), id));
    "create_workspace"(workspace: NewWorkspace) => async Reply::result(
        workspaces::create_workspace(State(&s.storage), State(&s.git), State(&s.herdr), workspace).await
    );
    "update_workspace"(workspace: Workspace) => sync Reply::result(
        workspaces::update_workspace(State(&s.storage), workspace)
    );
    "list_workspace_branches"(id: WorkspaceId) => sync Reply::result(
        workspaces::list_workspace_branches(State(&s.storage), State(&s.git), id)
    );
    "create_workspace_base_branch"(id: WorkspaceId, name: String, from: String) => sync Reply::result(
        workspaces::create_workspace_base_branch(State(&s.storage), State(&s.git), id, name, from)
    );
    "delete_workspace"(id: WorkspaceId) => sync Reply::result(workspaces::delete_workspace(State(&s.storage), id));
    "get_machine_settings"(machine: Machine) => sync Reply::result(
        workspaces::get_machine_settings(State(&s.storage), machine)
    );
    "save_machine_settings"(settings: MachineSettings) => sync Reply::result(
        workspaces::save_machine_settings(State(&s.storage), settings)
    );

    "list_herdr_machines"() => sync Reply::result(machines::list_herdr_machines(State(&s.herdr)));
    // `herdr machine add` asks questions in a terminal, which a server does not have.
    "add_herdr_machine"(host: String) => sync Reply::Failed(CommandError::rejected(format!(
        "the server cannot answer herdr's questions: run `herdr machine add {host}` in a \
         terminal on the server, then pick the machine here"
    )));
    "list_ssh_hosts"() => sync Reply::result(machines::list_ssh_hosts(State(&s.ssh_config)));
    "add_ssh_host"(host: NewSshHost) => sync Reply::result(machines::add_ssh_host(State(&s.ssh_config), host));
    "list_remote_directories"(machine: String, path: Option<String>) => async Reply::result(
        machines::list_remote_directories(State(&s.ssh), machine, path).await
    );
    "list_directories"(machine: Machine, path: Option<String>) => async Reply::result(
        machines::list_directories(State(&s.ssh), machine, path).await
    );

    "list_tasks"() => sync Reply::result(tasks::list_tasks(State(&s.storage)));
    "list_task_relations"() => sync Reply::result(tasks::list_task_relations(State(&s.storage)));
    "add_task_relation"(task_id: TaskId, related_id: TaskId) => sync Reply::result(
        tasks::add_task_relation(State(&s.storage), task_id, related_id)
    );
    "remove_task_relation"(task_id: TaskId, related_id: TaskId) => sync Reply::result(
        tasks::remove_task_relation(State(&s.storage), task_id, related_id)
    );
    "get_task"(id: TaskId) => sync Reply::result(tasks::get_task(State(&s.storage), id));
    "create_task"(task: NewTask) => sync Reply::result(
        tasks::create_task(State(&s.storage), State(&s.git), task)
    );
    "check_criterion"(task_id: TaskId, criterion_id: CriterionId, met: bool, evidence: Option<String>) => sync Reply::result(
        tasks::check_criterion(State(&s.storage), task_id, criterion_id, met, evidence)
    );
    "update_task"(task: TaskEdit) => sync Reply::result(tasks::update_task(State(&s.storage), task));
    "pause_task"(id: TaskId) => sync Reply::result(tasks::pause_task(State(&s.storage), id));
    "set_completion_note"(id: TaskId, note: String) => sync Reply::result(
        tasks::set_completion_note(State(&s.storage), id, note)
    );
    "delete_task"(id: TaskId, force: bool) => sync Reply::result(
        tasks::delete_task(State(&s.storage), State(&s.herdr), id, force)
    );
    "add_dependency"(task_id: TaskId, depends_on: TaskId) => sync Reply::result(
        tasks::add_dependency(State(&s.storage), State(&s.herdr), task_id, depends_on)
    );
    "remove_dependency"(task_id: TaskId, depends_on: TaskId) => sync Reply::result(
        tasks::remove_dependency(State(&s.storage), task_id, depends_on)
    );
    "list_commit_records"(task_id: TaskId) => sync Reply::result(
        tasks::list_commit_records(State(&s.storage), task_id)
    );

    "herdr_status"() => sync Reply::result(herdr::herdr_status(State(&s.herdr)));
    "start_task"(id: TaskId) => sync Reply::result(herdr::start_task(State(&s.storage), State(&s.git), State(&s.herdr), id));
    "ensure_task_workspace"(id: TaskId) => sync Reply::result(
        herdr::ensure_task_workspace(State(&s.storage), State(&s.herdr), id)
    );
    "open_task_in_herdr"(id: TaskId) => sync Reply::result(
        herdr::open_task_in_herdr(State(&s.storage), State(&s.herdr), id)
    );
    "focus_agent_pane"(id: TaskId) => sync Reply::result(
        herdr::focus_agent_pane(State(&s.storage), State(&s.herdr), id)
    );
    "restore_task_worktree"(id: TaskId) => sync Reply::result(
        herdr::restore_task_worktree(State(&s.storage), State(&s.git), State(&s.herdr), id)
    );
    "close_task_workspace"(id: TaskId) => sync Reply::result(
        herdr::close_task_workspace(State(&s.storage), State(&s.herdr), id)
    );

    "refresh_commit_records"(task_id: TaskId) => sync Reply::result(
        completion::refresh_commit_records(State(&s.storage), State(&s.git), task_id)
    );
    "open_pull_request"(id: TaskId) => sync Reply::result(
        completion::open_pull_request(State(&s.storage), State(&s.git), State(&s.github), id)
    );
    // A finish posts its report to the chat.
    "complete_task"(id: TaskId) => sync {
        let done = completion::complete_task(State(&s.storage), State(&s.git), State(&s.github), State(&s.herdr), id);
        if done.is_ok() {
            s.chat_changed();
        }
        Reply::result(done)
    };
    "conclude_task"(id: TaskId, note: String) => sync {
        let done = completion::conclude_task(State(&s.storage), State(&s.git), State(&s.github), State(&s.herdr), id, note);
        if done.is_ok() {
            s.chat_changed();
        }
        Reply::result(done)
    };
    "list_pull_request_statuses"() => sync Reply::value(
        completion::list_pull_request_statuses(State(&s.pull_request_statuses))
    );
    "branch_freshness"(id: TaskId) => sync Reply::result(
        freshness::branch_freshness(State(&s.storage), State(&s.git), State(&s.github), id)
    );
    "link_issue"(id: TaskId, number: Option<u32>) => sync Reply::result(
        issues::link_issue(State(&s.storage), State(&s.git), State(&s.github), id, number)
    );
    "create_issue"(id: TaskId) => sync Reply::result(
        issues::create_issue(State(&s.storage), State(&s.git), State(&s.github), id)
    );
    "list_agent_statuses"() => sync Reply::value(events::list_agent_statuses(State(&s.agent_statuses)));

    "mcp_server_info"() => sync Reply::value(mcp::mcp_server_info(State(&s.mcp)));
    "list_mcp_tunnels"() => sync Reply::value(mcp::list_mcp_tunnels(State(&s.tunnels)));

    "get_lead"() => sync Reply::value(lead::get_lead(State(&s.lead)));
    "list_chat_messages"() => sync Reply::result(lead::list_chat_messages(State(&s.storage)));
    "send_chat_message"(text: String, client_id: String) => sync {
        let sent = lead::send_chat_message(State(&s.storage), State(&s.herdr), State(&s.lead), text, client_id);
        if sent.is_ok() {
            s.chat_changed();
        }
        Reply::result(sent)
    };

    "review_period"(start: Timestamp, end: Timestamp) => sync Reply::result(
        review::review_period(State(&s.storage), start, end)
    );

    "get_backup_settings"() => sync Reply::value(backup::get_backup_settings(State(&s.backups)));
    "save_backup_settings"(settings: BackupSettings) => sync Reply::result(
        backup::save_backup_settings(State(&s.storage), State(&s.backups), settings)
    );
    "list_backups"() => sync Reply::result(backup::list_backups(State(&s.backups)));
    "restore_backup"(name: String) => sync Reply::result(
        backup::restore_backup(State(&s.storage), State(&s.backups), name)
    );

    "list_memory"(workspace_id: Option<WorkspaceId>, include_superseded: bool) => sync Reply::result(
        memory::list_memory(State(&s.storage), workspace_id, include_superseded)
    );
    "add_memory"(workspace_id: Option<WorkspaceId>, content: MemoryContent) => sync Reply::result(
        memory::add_memory(State(&s.storage), workspace_id, content)
    );
    "update_memory"(id: MemoryId, content: MemoryContent) => sync Reply::result(
        memory::update_memory(State(&s.storage), id, content)
    );
    "supersede_memory"(id: MemoryId, content: MemoryContent) => sync Reply::result(
        memory::supersede_memory(State(&s.storage), id, content)
    );
});

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Commands of the bindings that only the desktop app runs, because they read or write
    /// a path on the client's machine: the web app downloads reports in the browser and
    /// moves database files over `/api/backups` instead.
    const DESKTOP_ONLY: &[&str] = &[
        "export_database",
        "restore_database",
        "save_report",
        "set_connection",
        "get_access_token",
        "set_access_token",
    ];

    /// Every command the frontend's bindings invoke, from `src/lib/bindings.ts`.
    fn bound_commands() -> BTreeSet<String> {
        let bindings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/lib/bindings.ts"
        ))
        .unwrap();
        bindings
            .split("__TAURI_INVOKE")
            .skip(1)
            .filter_map(|call| {
                let call = call.strip_prefix('<').map_or(call, |rest| {
                    rest.split_once(">(").map_or(rest, |(_, after)| after)
                });
                let call = call.trim_start_matches('(');
                let name = call.strip_prefix('"')?.split('"').next()?;
                Some(name.to_owned())
            })
            .collect()
    }

    #[test]
    fn every_command_of_the_bindings_is_served() {
        let bound = bound_commands();
        assert!(
            bound.contains("list_tasks") && bound.contains("app_info"),
            "{bound:?}"
        );
        let served: BTreeSet<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();
        let desktop_only: BTreeSet<String> =
            DESKTOP_ONLY.iter().map(|name| (*name).to_owned()).collect();
        assert!(desktop_only.is_subset(&bound), "{desktop_only:?}");
        assert_eq!(served, &bound - &desktop_only);
    }

    #[test]
    fn arguments_use_the_camel_case_keys_of_tauri() {
        let args = serde_json::json!({ "taskId": 3, "dependsOn": 4 });
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            task_id: TaskId,
            depends_on: TaskId,
        }
        let Args {
            task_id,
            depends_on,
        } = parse::<Args>(args).unwrap();
        assert_eq!((task_id.0, depends_on.0), (3, 4));
        assert!(matches!(
            parse::<Args>(serde_json::json!({ "task_id": 3 })),
            Err(Reply::BadArguments(_))
        ));
    }
}
