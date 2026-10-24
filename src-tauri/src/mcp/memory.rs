//! What the project memory tools do: read and write the durable knowledge of a workspace
//! and the knowledge shared by every workspace, through the app's own memory commands.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::tools::present;
use crate::commands::memory::{self, scope};
use crate::commands::types::{
    self, MemoryContent, MemoryEntry, MemoryId, MemoryKind, TaskId, WorkspaceId,
};
use crate::commands::{CommandError, CommandResult};
use crate::storage::{Storage, StorageError};

const DEFAULT_SEARCH_LIMIT: u32 = 20;
const MAX_SEARCH_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ListMemory {
    /// The workspace whose entries to list; omit it for the shared entries alone.
    pub workspace_id: Option<WorkspaceId>,
    /// With workspace_id, also list the entries shared by every workspace.
    #[serde(default)]
    pub include_shared: bool,
    /// Also list entries that were superseded, the history.
    #[serde(default)]
    pub include_superseded: bool,
    /// Only entries of this kind.
    pub kind: Option<MemoryKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SearchMemory {
    /// Words that must all appear in an entry's title or body.
    pub query: String,
    /// The workspace to search; omit it to search the shared entries alone.
    pub workspace_id: Option<WorkspaceId>,
    /// With workspace_id, also search the entries shared by every workspace.
    #[serde(default)]
    pub include_shared: bool,
    /// Also search superseded entries.
    #[serde(default)]
    pub include_superseded: bool,
    /// At most this many entries (default 20, at most 100).
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GetMemory {
    pub id: MemoryId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AddMemory {
    /// The workspace the entry is about; omit it for knowledge shared by every workspace.
    pub workspace_id: Option<WorkspaceId>,
    pub kind: MemoryKind,
    /// One line that states the point, e.g. "Use npm scripts, not make".
    pub title: String,
    /// Details in GitHub-flavored Markdown: the why, the context, the alternatives.
    #[serde(default)]
    pub body: String,
    /// The task the knowledge came from; for a workspace entry, a task of that workspace.
    pub task_id: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct UpdateMemory {
    pub id: MemoryId,
    /// Unchanged when omitted.
    pub kind: Option<MemoryKind>,
    /// Unchanged when omitted.
    pub title: Option<String>,
    /// Unchanged when omitted.
    pub body: Option<String>,
    /// Unchanged when omitted; null clears it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<TaskId>")]
    pub task_id: Option<Option<TaskId>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SupersedeMemory {
    /// The current entry to replace.
    pub id: MemoryId,
    /// The old entry's kind when omitted.
    pub kind: Option<MemoryKind>,
    pub title: String,
    /// Markdown; say what changed and why.
    #[serde(default)]
    pub body: String,
    /// The old entry's task when omitted; null clears it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<TaskId>")]
    pub task_id: Option<Option<TaskId>>,
}

/// An entry with the entries it replaced, as get_memory returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDetails {
    pub entry: MemoryEntry,
    /// The name of the entry's workspace; null for a shared entry.
    pub workspace: Option<String>,
    /// The entries this one replaced, newest first.
    pub supersedes: Vec<MemoryEntry>,
}

fn find(storage: &Storage, id: MemoryId) -> CommandResult<crate::domain::memory::MemoryEntry> {
    Ok(storage
        .get_memory(id.into())?
        .ok_or(StorageError::MemoryNotFound(id.into()))?)
}

pub fn list_memory(storage: &Storage, request: ListMemory) -> CommandResult<Vec<MemoryEntry>> {
    let scope = scope(storage, request.workspace_id, request.include_shared)?;
    let entries = storage.list_memory(scope, request.include_superseded)?;
    Ok(entries
        .iter()
        .map(MemoryEntry::from)
        .filter(|entry| request.kind.is_none_or(|kind| entry.kind == kind))
        .collect())
}

pub fn search_memory(storage: &Storage, request: SearchMemory) -> CommandResult<Vec<MemoryEntry>> {
    if request.query.trim().is_empty() {
        return Err(CommandError::invalid_input("query must contain a word"));
    }
    let scope = scope(storage, request.workspace_id, request.include_shared)?;
    let limit = request
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    Ok(types::map_all(&storage.search_memory(
        &request.query,
        scope,
        request.include_superseded,
        limit,
    )?))
}

pub fn get_memory(storage: &Storage, request: GetMemory) -> CommandResult<MemoryDetails> {
    let entry = find(storage, request.id)?;
    let workspace = match entry.workspace {
        Some(id) => storage.get_workspace(id)?.map(|workspace| workspace.name),
        None => None,
    };
    Ok(MemoryDetails {
        entry: (&entry).into(),
        workspace,
        supersedes: types::map_all(&storage.memory_history(entry.id)?),
    })
}

pub fn add_memory(storage: &mut Storage, request: AddMemory) -> CommandResult<MemoryEntry> {
    memory::add(
        storage,
        request.workspace_id,
        MemoryContent {
            kind: request.kind,
            title: request.title,
            body: request.body,
            task_id: request.task_id,
        },
    )
}

pub fn update_memory(storage: &mut Storage, request: UpdateMemory) -> CommandResult<MemoryEntry> {
    let old = MemoryEntry::from(&find(storage, request.id)?);
    memory::update(
        storage,
        request.id,
        MemoryContent {
            kind: request.kind.unwrap_or(old.kind),
            title: request.title.unwrap_or(old.title),
            body: request.body.unwrap_or(old.body),
            task_id: request.task_id.unwrap_or(old.task_id),
        },
    )
}

pub fn supersede_memory(
    storage: &mut Storage,
    request: SupersedeMemory,
) -> CommandResult<MemoryEntry> {
    let old = MemoryEntry::from(&find(storage, request.id)?);
    memory::supersede(
        storage,
        request.id,
        MemoryContent {
            kind: request.kind.unwrap_or(old.kind),
            title: request.title,
            body: request.body,
            task_id: request.task_id.unwrap_or(old.task_id),
        },
    )
}
