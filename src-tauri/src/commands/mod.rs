//! Tauri commands exposed to the frontend. Arguments and results use the serializable
//! shapes in [`types`]; failures are reported as a [`CommandError`].

pub mod tasks;
pub mod types;
pub mod workspaces;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use specta::Type;

use crate::adapters::git::{Git, GitError};
use crate::domain::dag::DependencyError;
use crate::storage::{Storage, StorageError};

pub type StorageState = Mutex<Storage>;

pub type GitState = Arc<Git>;

pub type CommandResult<T> = Result<T, CommandError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CommandErrorKind {
    /// An argument is malformed, e.g. a relative path or an empty title.
    InvalidInput,
    /// A referenced workspace or task does not exist.
    NotFound,
    /// An external tool reported an error, e.g. git could not clone a repository or the
    /// path is not inside a git repository.
    External,
    /// The request is well-formed but breaks a rule, e.g. a dependency cycle or a
    /// forbidden status transition.
    Rejected,
    /// The database failed or holds data the app cannot read.
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct CommandError {
    pub kind: CommandErrorKind,
    pub message: String,
}

impl CommandError {
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            kind: CommandErrorKind::InvalidInput,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: CommandErrorKind::Internal,
            message: message.into(),
        }
    }
}

impl From<GitError> for CommandError {
    fn from(error: GitError) -> Self {
        let kind = match &error {
            GitError::Failed { .. } => CommandErrorKind::External,
            GitError::Spawn { .. } | GitError::UnexpectedOutput { .. } => {
                CommandErrorKind::Internal
            }
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

impl From<StorageError> for CommandError {
    fn from(error: StorageError) -> Self {
        let kind = match &error {
            StorageError::WorkspaceNotFound(_)
            | StorageError::TaskNotFound(_)
            | StorageError::Dependency(DependencyError::UnknownTask(_)) => {
                CommandErrorKind::NotFound
            }
            StorageError::Dependency(_)
            | StorageError::Transition(_)
            | StorageError::DerivedStatus(_) => CommandErrorKind::Rejected,
            StorageError::WorktreePathMismatch(_) | StorageError::CommitTaskMismatch { .. } => {
                CommandErrorKind::InvalidInput
            }
            StorageError::Sqlite(_)
            | StorageError::UnsupportedSchemaVersion { .. }
            | StorageError::InvalidData(_) => CommandErrorKind::Internal,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Locks the database. Every storage write runs in a transaction that rolls back when
/// dropped, so a panic in another command cannot leave partial writes behind and a
/// poisoned lock is safe to reuse.
fn lock(storage: &StorageState) -> MutexGuard<'_, Storage> {
    storage.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
}

#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::TaskId;
    use crate::domain::status::{TaskStatus, TransitionError};

    #[test]
    fn app_info_reports_crate_metadata() {
        let info = app_info();
        assert_eq!(info.name, "toodo");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn storage_errors_map_to_error_kinds() {
        let cases = [
            (
                StorageError::TaskNotFound(TaskId(1)),
                CommandErrorKind::NotFound,
            ),
            (
                StorageError::Dependency(DependencyError::UnknownTask(TaskId(1))),
                CommandErrorKind::NotFound,
            ),
            (
                StorageError::Dependency(DependencyError::Cycle(vec![TaskId(1), TaskId(1)])),
                CommandErrorKind::Rejected,
            ),
            (
                StorageError::Transition(TransitionError {
                    from: TaskStatus::Done,
                    to: TaskStatus::InProgress,
                }),
                CommandErrorKind::Rejected,
            ),
            (
                StorageError::WorktreePathMismatch(TaskId(1)),
                CommandErrorKind::InvalidInput,
            ),
            (
                StorageError::InvalidData("bad".to_owned()),
                CommandErrorKind::Internal,
            ),
        ];
        for (error, kind) in cases {
            let message = error.to_string();
            assert_eq!(CommandError::from(error), CommandError { kind, message });
        }
    }
}
