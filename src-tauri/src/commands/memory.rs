//! Project memory: what agents and the user keep about a workspace, or about every
//! workspace, between sessions. Entries change in place only for small fixes; a change
//! of meaning supersedes the entry, which keeps the old one as history.

use super::types::{MemoryContent, MemoryEntry, MemoryId, WorkspaceId};
use super::{CommandResult, State, StorageState, lock};
use crate::domain::memory::{self, MemoryScope};
use crate::storage::{Storage, StorageError};

/// `workspace` with or without the shared entries, or the shared entries alone.
pub(crate) fn scope(
    storage: &Storage,
    workspace: Option<WorkspaceId>,
    with_shared: bool,
) -> CommandResult<MemoryScope> {
    let Some(workspace) = workspace else {
        return Ok(MemoryScope::Shared);
    };
    storage
        .get_workspace(workspace.into())?
        .ok_or(StorageError::WorkspaceNotFound(workspace.into()))?;
    Ok(MemoryScope::Workspace {
        workspace: workspace.into(),
        with_shared,
    })
}

/// The entries of the workspace, or the shared ones when `workspace_id` is `null`; the
/// superseded ones too when `include_superseded`. Most recently changed first.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn list_memory(
    storage: State<'_, StorageState>,
    workspace_id: Option<WorkspaceId>,
    include_superseded: bool,
) -> CommandResult<Vec<MemoryEntry>> {
    let storage = lock(&storage);
    let scope = scope(&storage, workspace_id, false)?;
    Ok(super::types::map_all(
        &storage.list_memory(scope, include_superseded)?,
    ))
}

/// Adds an entry to the workspace, or a shared one when `workspace_id` is `null`.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn add_memory(
    storage: State<'_, StorageState>,
    workspace_id: Option<WorkspaceId>,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    add(&mut lock(&storage), workspace_id, content)
}

pub(crate) fn add(
    storage: &mut Storage,
    workspace_id: Option<WorkspaceId>,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    let content = memory::MemoryContent::try_from(content)?;
    Ok((&storage.add_memory(workspace_id.map(Into::into), &content)?).into())
}

/// Rewrites a current entry, e.g. to fix a mistake. Superseded entries cannot change.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn update_memory(
    storage: State<'_, StorageState>,
    id: MemoryId,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    update(&mut lock(&storage), id, content)
}

pub(crate) fn update(
    storage: &mut Storage,
    id: MemoryId,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    let content = memory::MemoryContent::try_from(content)?;
    Ok((&storage.update_memory(id.into(), &content)?).into())
}

/// Replaces a current entry with a new one in the same scope and returns the new entry;
/// the old one stays as history.
#[cfg_attr(feature = "desktop", tauri::command)]
#[cfg_attr(feature = "desktop", specta::specta)]
pub fn supersede_memory(
    storage: State<'_, StorageState>,
    id: MemoryId,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    supersede(&mut lock(&storage), id, content)
}

pub(crate) fn supersede(
    storage: &mut Storage,
    id: MemoryId,
    content: MemoryContent,
) -> CommandResult<MemoryEntry> {
    let content = memory::MemoryContent::try_from(content)?;
    Ok((&storage.supersede_memory(id.into(), &content)?).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;
    use crate::commands::types::MemoryKind;

    fn content(title: &str) -> MemoryContent {
        MemoryContent {
            kind: MemoryKind::Decision,
            title: title.to_owned(),
            body: String::new(),
            task_id: None,
        }
    }

    #[test]
    fn invalid_content_unknown_entries_and_superseded_entries_are_reported_by_kind() {
        let mut storage = Storage::open_in_memory().unwrap();
        let blank = add(&mut storage, None, content("  ")).unwrap_err();
        assert_eq!(blank.kind, CommandErrorKind::InvalidInput);
        let missing = add(&mut storage, Some(WorkspaceId(9)), content("x")).unwrap_err();
        assert_eq!(missing.kind, CommandErrorKind::NotFound);

        let first = add(&mut storage, None, content("Old")).unwrap();
        let second = supersede(&mut storage, first.id, content("New")).unwrap();
        assert_eq!(second.workspace_id, None);
        let error = update(&mut storage, first.id, content("Edit")).unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::Rejected);
        assert!(
            error.message.contains(&second.id.0.to_string()),
            "{error:?}"
        );
        let unknown = update(&mut storage, MemoryId(99), content("x")).unwrap_err();
        assert_eq!(unknown.kind, CommandErrorKind::NotFound);
    }
}
