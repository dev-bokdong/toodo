mod commands;
pub mod domain;
pub mod storage;

use std::sync::Mutex;

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

const DATABASE_FILE: &str = "toodo.sqlite3";

const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![commands::app_info])
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let storage = storage::Storage::open(&data_dir.join(DATABASE_FILE))?;
            app.manage(Mutex::new(storage));
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
