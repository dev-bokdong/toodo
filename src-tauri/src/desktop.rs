//! The desktop app (Windows, and macOS as a client of a server): the Tauri shell that
//! registers the commands, keeps their state, emits events to the frontend, and owns the
//! tray icon (the menu bar icon on macOS) and the MCP server. Connected
//! to a `toodo-server` (see [`connection`]), it keeps only the window and the tray: the
//! frontend sends its commands to the server, and no local data or background work starts.
//!
//! The same shell builds the Android app, which is always a client of a server: it has no
//! tray, updater, or local data, and adds links into the app.

use std::sync::{Arc, Mutex};

#[cfg(desktop)]
use tauri::menu::{IsMenuItem, Menu, MenuItem};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_specta::{Builder, Event, collect_commands, collect_events};

use crate::adapters::git::Git;
use crate::adapters::github::GitHub;
use crate::adapters::herdr::Herdr;
use crate::commands::events::{
    AgentStatusState, AgentStatusesChanged, MemoryChanged, Notifier, PullRequestStatusesChanged,
    TasksChanged, WorkspacesChanged, names,
};
use crate::commands::types::{TaskAgentStatus, TaskPullRequestStatus};
use crate::commands::{GitHubState, GitState, HerdrState, Runtime, StorageState};
use crate::domain::model;
use crate::{adapters, commands, connection, mcp, remote, storage};

const DATABASE_FILE: &str = "toodo.sqlite3";

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::app_info,
            commands::workspaces::list_workspaces,
            commands::workspaces::get_workspace,
            commands::workspaces::create_workspace,
            commands::workspaces::update_workspace,
            commands::workspaces::list_workspace_branches,
            commands::workspaces::create_workspace_base_branch,
            commands::workspaces::delete_workspace,
            commands::workspaces::get_machine_settings,
            commands::workspaces::save_machine_settings,
            commands::machines::list_herdr_machines,
            commands::machines::add_herdr_machine,
            commands::machines::list_ssh_hosts,
            commands::machines::add_ssh_host,
            commands::machines::list_remote_directories,
            commands::machines::list_directories,
            commands::tasks::list_tasks,
            commands::tasks::list_task_relations,
            commands::tasks::add_task_relation,
            commands::tasks::remove_task_relation,
            commands::tasks::get_task,
            commands::tasks::create_task,
            commands::tasks::update_task,
            commands::tasks::pause_task,
            commands::tasks::set_completion_note,
            commands::tasks::check_criterion,
            commands::tasks::delete_task,
            commands::tasks::add_dependency,
            commands::tasks::remove_dependency,
            commands::tasks::list_commit_records,
            commands::herdr::herdr_status,
            commands::herdr::start_task,
            commands::herdr::ensure_task_workspace,
            commands::herdr::open_task_in_herdr,
            commands::herdr::focus_agent_pane,
            commands::herdr::restore_task_worktree,
            commands::herdr::close_task_workspace,
            commands::completion::refresh_commit_records,
            commands::completion::open_pull_request,
            commands::completion::complete_task,
            commands::completion::conclude_task,
            commands::freshness::branch_freshness,
            commands::completion::list_pull_request_statuses,
            commands::issues::link_issue,
            commands::issues::create_issue,
            commands::events::list_agent_statuses,
            commands::mcp::mcp_server_info,
            commands::mcp::list_mcp_tunnels,
            commands::review::review_period,
            commands::review::save_report,
            commands::backup::get_backup_settings,
            commands::backup::save_backup_settings,
            commands::backup::list_backups,
            commands::backup::export_database,
            commands::backup::restore_database,
            commands::memory::list_memory,
            commands::memory::add_memory,
            commands::memory::update_memory,
            commands::memory::supersede_memory,
            commands::backup::restore_backup,
            connection::get_connection,
            connection::set_connection,
            connection::save_server,
            connection::remove_server,
            remote::access::get_access_token,
            remote::access::set_access_token,
        ])
        .events(collect_events![
            commands::events::TasksChanged,
            commands::events::WorkspacesChanged,
            commands::events::MemoryChanged,
            commands::events::AgentStatusesChanged,
            commands::events::PullRequestStatusesChanged,
            UpdateCheckRequested,
        ])
}

/// The user asked to check for an app update from the tray menu; the frontend runs the
/// check, since the updater is driven from there.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct UpdateCheckRequested;

const MAIN_WINDOW: &str = "main";

#[cfg(desktop)]
/// Whether the app checks for updates: releases carry updater artifacts for Windows only.
const UPDATES: bool = cfg!(not(target_os = "macos"));

#[cfg(desktop)]
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Adds the tray icon that keeps Toodo running after its window is closed.
#[cfg(desktop)]
fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Toodo", true, None::<&str>)?;
    let check_updates = UPDATES
        .then(|| {
            MenuItem::with_id(
                app,
                "check_updates",
                "Check for Updates",
                true,
                None::<&str>,
            )
        })
        .transpose()?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&show];
    if let Some(item) = &check_updates {
        items.push(item);
    }
    items.push(&quit);
    let menu = Menu::with_items(app, &items)?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Toodo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "check_updates" => {
                // Shown first so the result of the check is visible.
                show_main_window(app);
                if let Err(error) = UpdateCheckRequested.emit(app) {
                    eprintln!("toodo: failed to request an update check: {error}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if cfg!(target_os = "macos") {
        // A template image: macOS tints it to match the menu bar.
        tray = tray
            .icon(tauri::include_image!("icons/tray-template.png"))
            .icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Opens the main window of the configuration, told which server to use before its page
/// loads.
fn create_main_window(app: &tauri::App, connection: &connection::Connection) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW)
        .expect("tauri.conf.json defines the main window");
    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?
        .initialization_script(connection::init_script(connection))
        .initialization_script(platform_script())
        .build()?;
    Ok(())
}

/// Tells the page which system the app runs on, for what differs there: no update checks
/// where releases have no updater artifacts.
fn platform_script() -> String {
    let platform = serde_json::to_string(std::env::consts::OS).expect("a string serializes");
    format!("window.__TOODO_PLATFORM__ = {platform};")
}

fn on_run_event(app: &AppHandle, event: tauri::RunEvent) {
    // Clicking the Dock icon while the window is hidden brings it back.
    #[cfg(target_os = "macos")]
    if let tauri::RunEvent::Reopen { .. } = event {
        show_main_window(app);
    }
    shut_down_on_exit(app, event);
}

fn shut_down_on_exit(app: &AppHandle, event: tauri::RunEvent) {
    if let tauri::RunEvent::Exit = event
        && let Some(tunnels) = app.try_state::<mcp::tunnels::TunnelsState>()
    {
        tunnels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shut_down();
    }
}

#[cfg(desktop)]
fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)
        .expect("failed to export TypeScript bindings");
}

/// Runs the desktop app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    // The bindings are written into the source tree, which only a desktop build can reach.
    #[cfg(all(debug_assertions, desktop))]
    export_bindings(&builder);

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(remote::plugin());
    #[cfg(desktop)]
    let app = app
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            // Closing the main window hides it to the tray; Quit in the tray menu exits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                api.prevent_close();
                let _ = window.hide();
            }
        });
    #[cfg(mobile)]
    let app = app.plugin(tauri_plugin_deep_link::init());
    app.invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let connection = connection::load(&data_dir);
            #[cfg(desktop)]
            create_tray(app)?;
            app.manage(remote::access::AccessTokens::load(&data_dir));
            create_main_window(app, &connection)?;
            // The phone app keeps no data of its own; until it connects, it asks for a server.
            if cfg!(mobile) || connection.server.is_some() {
                return Ok(());
            }
            let storage = storage::Storage::open(&data_dir.join(DATABASE_FILE))?;
            app.manage::<commands::StorageState>(Mutex::new(storage));
            app.manage(commands::backup::Backups::load(&data_dir));
            app.manage::<commands::GitState>(Arc::new(adapters::git::Git::new(Box::new(
                adapters::git::ProcessRunner,
            ))));
            app.manage::<commands::GitHubState>(Arc::new(adapters::github::GitHub::new(Box::new(
                adapters::git::ProcessRunner,
            ))));
            app.manage::<commands::SshState>(Arc::new(adapters::ssh::Ssh::new(Box::new(
                adapters::git::ProcessRunner,
            ))));
            app.manage::<commands::SshConfigState>(adapters::ssh::SshConfigFile::in_home());
            app.manage::<commands::HerdrState>(Arc::new(adapters::herdr::Herdr::new(Box::new(
                adapters::git::ProcessRunner,
            ))));
            app.manage::<commands::events::AgentStatusState>(Mutex::default());
            app.manage::<commands::completion::PullRequestStatusState>(Mutex::default());
            let runtime: Arc<AppHandle> = Arc::new(app.handle().clone());
            commands::events::spawn(runtime.clone())?;
            commands::backup::spawn(runtime.clone())?;
            commands::completion::spawn(runtime.clone())?;
            let (mcp, server) = mcp::open(&data_dir);
            let tunnels: mcp::tunnels::TunnelsState = Arc::new(Mutex::new(
                mcp::tunnels::Tunnels::new(Box::new(adapters::ssh_tunnel::SshForwarder), mcp::PORT),
            ));
            match (&mcp.error, server) {
                (None, Some((listener, token))) => {
                    let router = mcp::router(runtime.clone(), token);
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = mcp::serve(listener, router).await {
                            eprintln!("toodo: MCP server stopped: {error}");
                        }
                    });
                    mcp::tunnels::spawn(runtime, tunnels.clone())?;
                }
                (error, _) => eprintln!(
                    "toodo: MCP server not started: {}",
                    error.as_deref().unwrap_or("unknown error")
                ),
            }
            app.manage(mcp);
            app.manage(tunnels);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(on_run_event);
}

impl Notifier for AppHandle {
    fn tasks_changed(&self, workspace: model::WorkspaceId) {
        let event = TasksChanged {
            workspace_id: workspace.into(),
        };
        if let Err(error) = event.emit(self) {
            eprintln!("toodo: failed to emit {}: {error}", names::TASKS_CHANGED);
        }
    }

    fn workspaces_changed(&self) {
        if let Err(error) = WorkspacesChanged.emit(self) {
            eprintln!(
                "toodo: failed to emit {}: {error}",
                names::WORKSPACES_CHANGED
            );
        }
    }

    fn memory_changed(&self) {
        if let Err(error) = MemoryChanged.emit(self) {
            eprintln!("toodo: failed to emit {}: {error}", names::MEMORY_CHANGED);
        }
    }

    fn agent_statuses_changed(&self, statuses: &[TaskAgentStatus]) {
        let event = AgentStatusesChanged {
            statuses: statuses.to_vec(),
        };
        if let Err(error) = event.emit(self) {
            eprintln!(
                "toodo: failed to emit {}: {error}",
                names::AGENT_STATUSES_CHANGED
            );
        }
    }

    fn pull_request_statuses_changed(&self, statuses: &[TaskPullRequestStatus]) {
        let event = PullRequestStatusesChanged {
            statuses: statuses.to_vec(),
        };
        if let Err(error) = event.emit(self) {
            eprintln!(
                "toodo: failed to emit {}: {error}",
                names::PULL_REQUEST_STATUSES_CHANGED
            );
        }
    }
}

impl mcp::Backend for AppHandle {
    fn storage(&self) -> &StorageState {
        self.state::<StorageState>().inner()
    }

    fn git(&self) -> &Git {
        self.state::<GitState>().inner()
    }

    fn github(&self) -> &GitHub {
        self.state::<GitHubState>().inner()
    }

    fn herdr(&self) -> &Herdr {
        self.state::<HerdrState>().inner()
    }
}

impl Runtime for AppHandle {
    fn agent_statuses(&self) -> &AgentStatusState {
        self.state::<AgentStatusState>().inner()
    }

    fn pull_request_statuses(&self) -> &commands::completion::PullRequestStatusState {
        self.state::<commands::completion::PullRequestStatusState>()
            .inner()
    }

    fn backups(&self) -> &commands::backup::Backups {
        self.state::<commands::backup::Backups>().inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerates `src/lib/bindings.ts`; run after changing any command signature.
    #[test]
    fn export_typescript_bindings() {
        export_bindings(&specta_builder());
    }

    /// The server sends events under these names, so they must match the desktop ones.
    #[test]
    fn event_names_match_the_server_ones() {
        assert_eq!(TasksChanged::NAME, names::TASKS_CHANGED);
        assert_eq!(WorkspacesChanged::NAME, names::WORKSPACES_CHANGED);
        assert_eq!(MemoryChanged::NAME, names::MEMORY_CHANGED);
        assert_eq!(AgentStatusesChanged::NAME, names::AGENT_STATUSES_CHANGED);
        assert_eq!(
            PullRequestStatusesChanged::NAME,
            names::PULL_REQUEST_STATUSES_CHANGED
        );
    }
}
