//! Commands for database backups: manual export, restore, the automatic backup schedule,
//! and the thread that writes automatic backups.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};

use super::types::{self, BackupEntry, BackupSettings};
use super::{CommandError, CommandResult, StorageState, lock};
use crate::domain::backup::{self, BackupKind};
use crate::storage::{self, BackupFile};

const BACKUP_DIR: &str = "backups";
const SETTINGS_FILE: &str = "backup-settings.json";

/// How often the backup thread checks whether an automatic backup is due.
const CHECK_PERIOD: Duration = Duration::from_secs(10 * 60);

/// Toodo's backup directory and automatic backup settings. The settings live in a file
/// next to the database rather than in it, so a restore does not change them.
pub struct Backups {
    dir: PathBuf,
    settings_path: PathBuf,
    settings: Mutex<backup::BackupSettings>,
}

impl Backups {
    /// Uses `<data_dir>/backups` and reads the settings saved in `data_dir`; missing or
    /// unreadable settings fall back to the defaults.
    pub fn load(data_dir: &Path) -> Self {
        let settings_path = data_dir.join(SETTINGS_FILE);
        let settings = read_settings(&settings_path).unwrap_or_else(|error| {
            eprintln!(
                "toodo: ignoring backup settings in {}: {error}",
                settings_path.display()
            );
            backup::BackupSettings::default()
        });
        Self {
            dir: data_dir.join(BACKUP_DIR),
            settings_path,
            settings: Mutex::new(settings),
        }
    }

    fn settings(&self) -> backup::BackupSettings {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save_settings(&self, settings: backup::BackupSettings) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&BackupSettings::from(settings))
            .map_err(io::Error::other)?;
        let mut current = self.settings.lock().unwrap_or_else(PoisonError::into_inner);
        fs::write(&self.settings_path, json)?;
        *current = settings;
        Ok(())
    }
}

fn read_settings(path: &Path) -> Result<backup::BackupSettings, String> {
    let json = match fs::read_to_string(path) {
        Ok(json) => json,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(backup::BackupSettings::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    let settings: BackupSettings =
        serde_json::from_str(&json).map_err(|error| error.to_string())?;
    backup::BackupSettings::try_from(settings).map_err(|error| error.message)
}

fn absolute_path(path: &str) -> CommandResult<&Path> {
    let path = Path::new(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CommandError::invalid_input(format!(
            "{} is not an absolute path",
            path.display()
        )))
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_backup_settings(backups: State<'_, Backups>) -> BackupSettings {
    backups.settings().into()
}

/// Saves the automatic backup settings, then writes an automatic backup if one is now
/// due.
#[tauri::command]
#[specta::specta]
pub fn save_backup_settings(
    storage: State<'_, StorageState>,
    backups: State<'_, Backups>,
    settings: BackupSettings,
) -> CommandResult<BackupSettings> {
    let settings = backup::BackupSettings::try_from(settings)?;
    backups.save_settings(settings).map_err(|error| {
        CommandError::internal(format!("failed to save backup settings: {error}"))
    })?;
    run_due_backup(&storage, &backups)?;
    Ok(settings.into())
}

/// Backups in Toodo's backup directory, newest first.
#[tauri::command]
#[specta::specta]
pub fn list_backups(backups: State<'_, Backups>) -> CommandResult<Vec<BackupEntry>> {
    Ok(types::map_all(&storage::list_backups(&backups.dir)?))
}

/// Writes a snapshot of the database to `path`, replacing any file there.
#[tauri::command]
#[specta::specta]
pub fn export_database(storage: State<'_, StorageState>, path: String) -> CommandResult<()> {
    Ok(lock(&storage).export_to(absolute_path(&path)?)?)
}

/// Replaces all data with the backup at `path`. The current data is first saved as a
/// pre-restore backup, which is returned.
#[tauri::command]
#[specta::specta]
pub fn restore_database(
    storage: State<'_, StorageState>,
    backups: State<'_, Backups>,
    path: String,
) -> CommandResult<BackupEntry> {
    Ok((&restore(&storage, &backups, absolute_path(&path)?)?).into())
}

fn restore(storage: &StorageState, backups: &Backups, path: &Path) -> storage::Result<BackupFile> {
    let staged = storage::stage_backup(path)?;
    let mut storage = lock(storage);
    let safety = storage.backup_into(&backups.dir, BackupKind::PreRestore)?;
    storage.restore(staged)?;
    Ok(safety)
}

/// Writes an automatic backup when the settings enable it and the interval has passed
/// since the latest one, then deletes the automatic backups beyond the number kept.
fn run_due_backup(
    storage: &StorageState,
    backups: &Backups,
) -> storage::Result<Option<BackupFile>> {
    let settings = backups.settings();
    if !settings.enabled {
        return Ok(None);
    }
    let storage = lock(storage);
    let latest = storage::list_backups(&backups.dir)?
        .into_iter()
        .find(|file| file.kind == BackupKind::Automatic)
        .map(|file| file.created_at);
    if !backup::is_due(latest, storage.now(), settings.interval_hours) {
        return Ok(None);
    }
    let file = storage.backup_into(&backups.dir, BackupKind::Automatic)?;
    drop(storage);
    let keep = usize::try_from(settings.keep).expect("kept backup count fits in usize");
    storage::prune_backups(&backups.dir, BackupKind::Automatic, keep)?;
    Ok(Some(file))
}

/// Starts the thread that writes automatic backups while the app runs.
pub fn spawn(app: AppHandle) -> io::Result<()> {
    thread::Builder::new()
        .name("auto-backup".to_owned())
        .spawn(move || {
            loop {
                if let Err(error) = run_due_backup(&app.state::<StorageState>(), &app.state()) {
                    eprintln!("toodo: automatic backup failed: {error}");
                }
                thread::sleep(CHECK_PERIOD);
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::WorkspaceLocation;
    use crate::domain::path::WindowsPath;
    use crate::storage::{NewWorkspace, Storage};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    const HOUR: i64 = 3_600;

    struct Fixture {
        _dir: tempfile::TempDir,
        data_dir: PathBuf,
        storage: StorageState,
        clock: Arc<AtomicI64>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let data_dir = dir.path().to_owned();
            let mut storage = Storage::open(&data_dir.join("toodo.sqlite3")).unwrap();
            let clock = Arc::new(AtomicI64::new(1_791_970_205));
            let shared = clock.clone();
            storage.set_clock(move || shared.load(Ordering::SeqCst));
            Self {
                _dir: dir,
                data_dir,
                storage: Mutex::new(storage),
                clock,
            }
        }

        fn advance(&self, seconds: i64) {
            self.clock.fetch_add(seconds, Ordering::SeqCst);
        }

        fn add_workspace(&self, name: &str) {
            lock(&self.storage)
                .create_workspace(&NewWorkspace {
                    name: name.to_owned(),
                    location: WorkspaceLocation::Local {
                        path: WindowsPath::parse(&format!("C:\\dev\\{name}")).unwrap(),
                    },
                    default_base_branch: "main".to_owned(),
                })
                .unwrap();
        }

        fn workspace_names(&self) -> Vec<String> {
            lock(&self.storage)
                .list_workspaces()
                .unwrap()
                .into_iter()
                .map(|workspace| workspace.name)
                .collect()
        }
    }

    fn settings(enabled: bool, interval_hours: u32, keep: u32) -> backup::BackupSettings {
        backup::BackupSettings {
            enabled,
            interval_hours,
            keep,
        }
    }

    #[test]
    fn automatic_backups_follow_the_interval_and_keep_the_newest() {
        let fixture = Fixture::new();
        let backups = Backups::load(&fixture.data_dir);
        backups.save_settings(settings(true, 2, 2)).unwrap();
        fixture.add_workspace("toodo");

        let first = run_due_backup(&fixture.storage, &backups).unwrap().unwrap();
        assert!(first.path.exists());
        fixture.advance(HOUR);
        assert_eq!(run_due_backup(&fixture.storage, &backups).unwrap(), None);
        fixture.advance(HOUR);
        let second = run_due_backup(&fixture.storage, &backups).unwrap().unwrap();
        fixture.advance(2 * HOUR);
        let third = run_due_backup(&fixture.storage, &backups).unwrap().unwrap();

        let kept: Vec<PathBuf> = storage::list_backups(&backups.dir)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(kept, vec![third.path, second.path]);
        assert!(!first.path.exists());
    }

    #[test]
    fn disabled_automatic_backups_write_nothing() {
        let fixture = Fixture::new();
        let backups = Backups::load(&fixture.data_dir);
        backups.save_settings(settings(false, 1, 7)).unwrap();
        assert_eq!(run_due_backup(&fixture.storage, &backups).unwrap(), None);
        assert!(storage::list_backups(&backups.dir).unwrap().is_empty());
    }

    #[test]
    fn pruning_keeps_pre_restore_backups() {
        let fixture = Fixture::new();
        let backups = Backups::load(&fixture.data_dir);
        backups.save_settings(settings(true, 1, 1)).unwrap();
        let export = fixture.data_dir.join("export.sqlite3");
        lock(&fixture.storage).export_to(&export).unwrap();
        let safety = restore(&fixture.storage, &backups, &export).unwrap();
        fixture.advance(HOUR);
        run_due_backup(&fixture.storage, &backups).unwrap().unwrap();
        fixture.advance(HOUR);
        let latest = run_due_backup(&fixture.storage, &backups).unwrap().unwrap();

        let kept: Vec<PathBuf> = storage::list_backups(&backups.dir)
            .unwrap()
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(kept, vec![latest.path, safety.path]);
    }

    #[test]
    fn restore_saves_the_current_data_first() {
        let fixture = Fixture::new();
        let backups = Backups::load(&fixture.data_dir);
        fixture.add_workspace("before");
        let export = fixture.data_dir.join("export.sqlite3");
        lock(&fixture.storage).export_to(&export).unwrap();
        fixture.add_workspace("after");

        let safety = restore(&fixture.storage, &backups, &export).unwrap();

        assert_eq!(safety.kind, BackupKind::PreRestore);
        assert_eq!(fixture.workspace_names(), ["before"]);
        restore(&fixture.storage, &backups, &safety.path).unwrap();
        assert_eq!(fixture.workspace_names(), ["before", "after"]);
    }

    #[test]
    fn an_invalid_backup_leaves_data_and_backups_alone() {
        let fixture = Fixture::new();
        let backups = Backups::load(&fixture.data_dir);
        fixture.add_workspace("toodo");
        let garbage = fixture.data_dir.join("garbage.sqlite3");
        fs::write(
            &garbage,
            b"not a database at all, just some bytes".repeat(200),
        )
        .unwrap();

        let error = restore(&fixture.storage, &backups, &garbage).unwrap_err();

        assert!(
            matches!(error, storage::StorageError::InvalidBackup(_)),
            "{error}"
        );
        assert_eq!(fixture.workspace_names(), ["toodo"]);
        assert!(storage::list_backups(&backups.dir).unwrap().is_empty());
    }

    #[test]
    fn settings_are_saved_to_a_file_and_read_back() {
        let fixture = Fixture::new();
        assert_eq!(
            Backups::load(&fixture.data_dir).settings(),
            backup::BackupSettings::default()
        );
        Backups::load(&fixture.data_dir)
            .save_settings(settings(false, 6, 3))
            .unwrap();
        assert_eq!(
            Backups::load(&fixture.data_dir).settings(),
            settings(false, 6, 3)
        );

        fs::write(
            fixture.data_dir.join(SETTINGS_FILE),
            r#"{"enabled":true,"intervalHours":0,"keep":3}"#,
        )
        .unwrap();
        assert_eq!(
            Backups::load(&fixture.data_dir).settings(),
            backup::BackupSettings::default()
        );
    }

    #[test]
    fn exports_need_an_absolute_path() {
        assert_eq!(
            absolute_path("backup.sqlite3").unwrap_err().kind,
            super::super::CommandErrorKind::InvalidInput
        );
    }
}
