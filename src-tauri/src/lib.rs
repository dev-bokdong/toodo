mod adapters;
mod commands;
pub mod domain;
pub mod storage;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

const DATABASE_FILE: &str = "toodo.sqlite3";

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
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
        commands::tasks::set_task_status,
        commands::tasks::delete_task,
        commands::tasks::add_dependency,
        commands::tasks::remove_dependency,
        commands::tasks::list_commit_records,
        commands::herdr::herdr_status,
        commands::herdr::start_task,
        commands::herdr::open_task_in_herdr,
        commands::completion::refresh_commit_records,
        commands::completion::complete_task,
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let storage = storage::Storage::open(&data_dir.join(DATABASE_FILE))?;
            app.manage::<commands::StorageState>(Mutex::new(storage));
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
