mod adapters;
mod commands;
pub mod domain;
mod mcp;
pub mod storage;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri_specta::{Builder, collect_commands, collect_events};

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
            commands::tasks::list_tasks,
            commands::tasks::get_task,
            commands::tasks::create_task,
            commands::tasks::update_task,
            commands::tasks::pause_task,
            commands::tasks::set_completion_note,
            commands::tasks::delete_task,
            commands::tasks::add_dependency,
            commands::tasks::remove_dependency,
            commands::tasks::list_commit_records,
            commands::herdr::herdr_status,
            commands::herdr::start_task,
            commands::herdr::open_task_in_herdr,
            commands::herdr::restore_task_worktree,
            commands::completion::refresh_commit_records,
            commands::completion::open_pull_request,
            commands::completion::complete_task,
            commands::completion::conclude_task,
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
        ])
        .events(collect_events![
            commands::events::TasksChanged,
            commands::events::AgentStatusesChanged,
            commands::events::PullRequestStatusesChanged,
        ])
}

const MAIN_WINDOW: &str = "main";

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Adds the tray icon that keeps Toodo running after its window is closed.
fn create_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Toodo", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Toodo")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
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
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn export_bindings(builder: &Builder<tauri::Wry>) {
    builder
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)
        .expect("failed to export TypeScript bindings");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .on_window_event(|window, event| {
            // Closing the main window hides it to the tray; Quit in the tray menu exits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == MAIN_WINDOW
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            builder.mount_events(app);
            create_tray(app)?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
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
            commands::events::spawn(app.handle().clone())?;
            commands::backup::spawn(app.handle().clone())?;
            commands::completion::spawn(app.handle().clone())?;
            let mcp = mcp::start(Arc::new(app.handle().clone()), &data_dir);
            let tunnels: mcp::tunnels::TunnelsState = Arc::new(Mutex::new(
                mcp::tunnels::Tunnels::new(Box::new(adapters::ssh_tunnel::SshForwarder), mcp::PORT),
            ));
            match &mcp.error {
                Some(error) => eprintln!("toodo: MCP server not started: {error}"),
                None => mcp::tunnels::spawn(app.handle().clone(), tunnels.clone())?,
            }
            app.manage(mcp);
            app.manage(tunnels);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(tunnels) = app.try_state::<mcp::tunnels::TunnelsState>()
            {
                tunnels
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .shut_down();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regenerates `src/lib/bindings.ts`; run after changing any command signature.
    #[test]
    fn export_typescript_bindings() {
        export_bindings(&specta_builder());
    }
}
