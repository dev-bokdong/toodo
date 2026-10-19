//! SQLite persistence for workspaces, machine settings, tasks, task dependencies, and
//! commit records.
//!
//! The database lives in a single file in the app data directory. Its schema is
//! versioned with `PRAGMA user_version` and migrated forward when it is opened. Writes
//! that touch the task DAG run in one transaction and keep it valid: dependency edges
//! are checked with [`crate::domain::dag`], and `planned`/`ready` statuses are brought in
//! line with the dependencies after every change.

mod backup;
mod commits;
mod migrations;
mod search;
mod tasks;
mod workspaces;

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::domain::dag::DependencyError;
use crate::domain::model::{TaskId, WorkspaceId};
use crate::domain::status::TransitionError;

pub use backup::{BackupFile, StagedBackup, list_backups, prune_backups, stage_backup};
pub use search::TaskSearch;
pub use tasks::{AddedDependencies, NewTask};
pub use workspaces::NewWorkspace;

pub type Result<T> = std::result::Result<T, StorageError>;

/// Handle to the Toodo database. Readers take `&self`; writers take `&mut self` and run
/// in a transaction.
pub struct Storage {
    conn: Connection,
    /// Current time in seconds since the Unix epoch, used to timestamp task status
    /// changes.
    clock: Box<dyn Fn() -> i64 + Send>,
}

fn system_clock() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is set after the Unix epoch");
    i64::try_from(elapsed.as_secs()).expect("seconds since the Unix epoch fit in i64")
}

impl Storage {
    /// Opens (or creates) the database file at `path` and migrates it to the latest
    /// schema.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Opens a private in-memory database with the latest schema.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        migrations::migrate(&mut conn)?;
        conn.pragma_update(None, "foreign_keys", true)?;
        Ok(Self {
            conn,
            clock: Box::new(system_clock),
        })
    }

    /// Replaces the clock that timestamps task status changes.
    #[cfg(test)]
    pub(crate) fn set_clock(&mut self, clock: impl Fn() -> i64 + Send + 'static) {
        self.clock = Box::new(clock);
    }

    pub(crate) fn now(&self) -> i64 {
        (self.clock)()
    }
}

#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    /// The database was written by a newer version of Toodo.
    UnsupportedSchemaVersion {
        found: i64,
        supported: i64,
    },
    WorkspaceNotFound(WorkspaceId),
    /// A workspace cannot be deleted while a task of another workspace still waits on
    /// one of its tasks: deleting the prerequisite would make the dependent ready.
    WorkspaceHasDependents {
        workspace: WorkspaceId,
        task: TaskId,
        depends_on: TaskId,
    },
    /// The base branch of a workspace cannot change while one of its tasks is in review,
    /// because the task's pull request targets the current branch.
    BaseBranchInReview {
        workspace: WorkspaceId,
        task: TaskId,
    },
    TaskNotFound(TaskId),
    Dependency(DependencyError),
    Transition(TransitionError),
    /// A task cannot start while one of its dependencies is not done.
    DependenciesNotDone(TaskId),
    /// A task cannot be related to itself.
    SelfRelation(TaskId),
    /// The worktree path is a Windows path for an SSH workspace or a POSIX path for a
    /// local one.
    WorktreePathMismatch(TaskId),
    CommitTaskMismatch {
        expected: TaskId,
        found: TaskId,
    },
    /// A stored value cannot be converted back into the domain model.
    InvalidData(String),
    /// A file to restore from is not a readable Toodo database.
    InvalidBackup(String),
    Io(std::io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "database error: {error}"),
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "database schema version {found} is newer than the supported version {supported}"
            ),
            Self::WorkspaceNotFound(id) => write!(f, "workspace {} does not exist", id.0),
            Self::WorkspaceHasDependents {
                workspace,
                task,
                depends_on,
            } => write!(
                f,
                "workspace {} cannot be removed: task {task} of another workspace depends on its task {depends_on}",
                workspace.0
            ),
            Self::BaseBranchInReview { workspace, task } => write!(
                f,
                "base branch of workspace {} cannot change while task {task} is in review",
                workspace.0
            ),
            Self::TaskNotFound(id) => write!(f, "task {id} does not exist"),
            Self::Dependency(error) => error.fmt(f),
            Self::Transition(error) => error.fmt(f),
            Self::DependenciesNotDone(id) => {
                write!(
                    f,
                    "task {id} cannot start before all its dependencies are done"
                )
            }
            Self::SelfRelation(id) => write!(f, "task {id} cannot be related to itself"),
            Self::WorktreePathMismatch(id) => write!(
                f,
                "worktree path of task {id} does not match the machine of its workspace"
            ),
            Self::CommitTaskMismatch { expected, found } => write!(
                f,
                "commit record of task {found} cannot be stored for task {expected}"
            ),
            Self::InvalidData(message) => write!(f, "stored data is invalid: {message}"),
            Self::InvalidBackup(message) => write!(f, "invalid backup {message}"),
            Self::Io(error) => write!(f, "file error: {error}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Dependency(error) => Some(error),
            Self::Transition(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<DependencyError> for StorageError {
    fn from(error: DependencyError) -> Self {
        Self::Dependency(error)
    }
}

impl From<TransitionError> for StorageError {
    fn from(error: TransitionError) -> Self {
        Self::Transition(error)
    }
}

fn invalid_data(error: impl fmt::Display) -> StorageError {
    StorageError::InvalidData(error.to_string())
}
