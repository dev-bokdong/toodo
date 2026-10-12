//! SQLite persistence for workspaces, machine settings, tasks, task dependencies, and
//! commit records.
//!
//! The database lives in a single file in the app data directory. Its schema is
//! versioned with `PRAGMA user_version` and migrated forward when it is opened. Writes
//! that touch the task DAG run in one transaction and keep it valid: dependency edges
//! are checked with [`crate::domain::dag`], and `planned`/`ready` statuses are brought in
//! line with the dependencies after every change.

mod commits;
mod migrations;
mod tasks;
mod workspaces;

#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::domain::dag::DependencyError;
use crate::domain::model::{TaskId, WorkspaceId};
use crate::domain::status::{TaskStatus, TransitionError};

pub use tasks::NewTask;
pub use workspaces::NewWorkspace;

pub type Result<T> = std::result::Result<T, StorageError>;

/// Handle to the Toodo database. Readers take `&self`; writers take `&mut self` and run
/// in a transaction.
pub struct Storage {
    conn: Connection,
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
        Ok(Self { conn })
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
    TaskNotFound(TaskId),
    Dependency(DependencyError),
    Transition(TransitionError),
    /// `planned` and `ready` follow from a task's dependencies and cannot be set directly.
    DerivedStatus(TaskStatus),
    /// The worktree path is a Windows path for an SSH workspace or a POSIX path for a
    /// local one.
    WorktreePathMismatch(TaskId),
    CommitTaskMismatch {
        expected: TaskId,
        found: TaskId,
    },
    /// A stored value cannot be converted back into the domain model.
    InvalidData(String),
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
            Self::TaskNotFound(id) => write!(f, "task {id} does not exist"),
            Self::Dependency(error) => error.fmt(f),
            Self::Transition(error) => error.fmt(f),
            Self::DerivedStatus(status) => write!(
                f,
                "status {status} is derived from dependencies and cannot be set directly"
            ),
            Self::WorktreePathMismatch(id) => write!(
                f,
                "worktree path of task {id} does not match the machine of its workspace"
            ),
            Self::CommitTaskMismatch { expected, found } => write!(
                f,
                "commit record of task {found} cannot be stored for task {expected}"
            ),
            Self::InvalidData(message) => write!(f, "stored data is invalid: {message}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Dependency(error) => Some(error),
            Self::Transition(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
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
