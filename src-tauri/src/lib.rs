mod adapters;
mod commands;
pub mod domain;
pub mod storage;

use std::sync::{Arc, Mutex};

use tauri::Manager;
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
            commands::events::list_agent_statuses,
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
        ])
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
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
            commands::events::spawn(app.handle().clone())?;
            commands::backup::spawn(app.handle().clone())?;
            commands::completion::spawn(app.handle().clone())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
