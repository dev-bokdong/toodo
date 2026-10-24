//! Project memory entries of each workspace and the ones shared by all of them.

use rusqlite::{Connection, OptionalExtension, Row, ToSql};

use super::search::like_pattern;
use super::{Result, Storage, StorageError, invalid_data};
use crate::domain::memory::{MemoryContent, MemoryEntry, MemoryId, MemoryKind, MemoryScope};
use crate::domain::model::{TaskId, WorkspaceId};

const COLUMNS: &str = "id, workspace_id, kind, title, body, task_id, created_at, updated_at, \
    superseded_by";

fn entry(row: &Row<'_>) -> rusqlite::Result<(MemoryEntry, String)> {
    let kind: String = row.get(2)?;
    Ok((
        MemoryEntry {
            id: MemoryId(row.get(0)?),
            workspace: row.get::<_, Option<i64>>(1)?.map(WorkspaceId),
            kind: MemoryKind::Note,
            title: row.get(3)?,
            body: row.get(4)?,
            task: row.get::<_, Option<i64>>(5)?.map(TaskId),
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
            superseded_by: row.get::<_, Option<i64>>(8)?.map(MemoryId),
        },
        kind,
    ))
}

fn with_kind((mut entry, kind): (MemoryEntry, String)) -> Result<MemoryEntry> {
    entry.kind = MemoryKind::parse(&kind)
        .ok_or_else(|| invalid_data(format!("unknown memory kind {kind}")))?;
    Ok(entry)
}

fn query(conn: &Connection, sql: &str, parameters: &[Box<dyn ToSql>]) -> Result<Vec<MemoryEntry>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement
        .query_map(
            rusqlite::params_from_iter(parameters.iter().map(Box::as_ref)),
            entry,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(with_kind).collect()
}

fn find(conn: &Connection, id: MemoryId) -> Result<Option<MemoryEntry>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM memory_entries WHERE id = ?1"),
        [id.0],
        entry,
    )
    .optional()?
    .map(with_kind)
    .transpose()
}

/// The entry `id`, which must still be current to change.
fn current(conn: &Connection, id: MemoryId) -> Result<MemoryEntry> {
    let entry = find(conn, id)?.ok_or(StorageError::MemoryNotFound(id))?;
    match entry.superseded_by {
        Some(by) => Err(StorageError::MemorySuperseded { id, by }),
        None => Ok(entry),
    }
}

/// Rejects an unknown workspace, and a task that does not exist or, for a workspace
/// entry, belongs to another workspace. Shared entries may name a task of any workspace.
fn check_links(
    conn: &Connection,
    workspace: Option<WorkspaceId>,
    task: Option<TaskId>,
) -> Result<()> {
    if let Some(workspace) = workspace {
        conn.query_row(
            "SELECT 1 FROM workspaces WHERE id = ?1",
            [workspace.0],
            |_| Ok(()),
        )
        .optional()?
        .ok_or(StorageError::WorkspaceNotFound(workspace))?;
    }
    if let Some(task) = task {
        let owner: i64 = conn
            .query_row(
                "SELECT workspace_id FROM tasks WHERE id = ?1",
                [task.0],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::TaskNotFound(task))?;
        if let Some(workspace) = workspace
            && workspace.0 != owner
        {
            return Err(StorageError::MemoryTaskMismatch { task, workspace });
        }
    }
    Ok(())
}

fn insert(
    conn: &Connection,
    workspace: Option<WorkspaceId>,
    content: &MemoryContent,
    at: i64,
) -> Result<MemoryId> {
    conn.execute(
        "INSERT INTO memory_entries
             (workspace_id, kind, title, body, task_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        rusqlite::params![
            workspace.map(|id| id.0),
            content.kind.as_str(),
            content.title,
            content.body,
            content.task.map(|id| id.0),
            at
        ],
    )?;
    Ok(MemoryId(conn.last_insert_rowid()))
}

/// The `WHERE` conditions selecting the entries of `scope`, current ones only unless
/// `include_superseded`.
fn scope_conditions(
    scope: MemoryScope,
    include_superseded: bool,
    parameters: &mut Vec<Box<dyn ToSql>>,
) -> Vec<String> {
    let mut conditions = vec![match scope {
        MemoryScope::Shared => "workspace_id IS NULL".to_owned(),
        MemoryScope::Workspace {
            workspace,
            with_shared,
        } => {
            parameters.push(Box::new(workspace.0));
            if with_shared {
                "(workspace_id = ? OR workspace_id IS NULL)".to_owned()
            } else {
                "workspace_id = ?".to_owned()
            }
        }
    }];
    if !include_superseded {
        conditions.push("superseded_by IS NULL".to_owned());
    }
    conditions
}

impl Storage {
    /// Adds an entry to `workspace`, or a shared one when `None`.
    pub fn add_memory(
        &mut self,
        workspace: Option<WorkspaceId>,
        content: &MemoryContent,
    ) -> Result<MemoryEntry> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        check_links(&tx, workspace, content.task)?;
        let id = insert(&tx, workspace, content, at)?;
        let entry = find(&tx, id)?.ok_or(StorageError::MemoryNotFound(id))?;
        tx.commit()?;
        Ok(entry)
    }

    /// Rewrites a current entry in place, e.g. to fix a typo; a change of meaning should
    /// supersede it instead, which keeps the old text.
    pub fn update_memory(&mut self, id: MemoryId, content: &MemoryContent) -> Result<MemoryEntry> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let old = current(&tx, id)?;
        check_links(&tx, old.workspace, content.task)?;
        tx.execute(
            "UPDATE memory_entries
             SET kind = ?2, title = ?3, body = ?4, task_id = ?5, updated_at = ?6
             WHERE id = ?1",
            rusqlite::params![
                id.0,
                content.kind.as_str(),
                content.title,
                content.body,
                content.task.map(|id| id.0),
                at
            ],
        )?;
        let entry = find(&tx, id)?.ok_or(StorageError::MemoryNotFound(id))?;
        tx.commit()?;
        Ok(entry)
    }

    /// Replaces a current entry with a new one in the same scope and returns the new
    /// entry. The old entry is kept unchanged, except that it now points at its
    /// replacement.
    pub fn supersede_memory(
        &mut self,
        id: MemoryId,
        content: &MemoryContent,
    ) -> Result<MemoryEntry> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let old = current(&tx, id)?;
        check_links(&tx, old.workspace, content.task)?;
        let new = insert(&tx, old.workspace, content, at)?;
        tx.execute(
            "UPDATE memory_entries SET superseded_by = ?2 WHERE id = ?1",
            [id.0, new.0],
        )?;
        let entry = find(&tx, new)?.ok_or(StorageError::MemoryNotFound(new))?;
        tx.commit()?;
        Ok(entry)
    }

    pub fn get_memory(&self, id: MemoryId) -> Result<Option<MemoryEntry>> {
        find(&self.conn, id)
    }

    /// The entries `id` replaced, directly or through earlier replacements, newest
    /// first.
    pub fn memory_history(&self, id: MemoryId) -> Result<Vec<MemoryEntry>> {
        let sql = format!(
            "WITH RECURSIVE older (id, depth) AS (
                 SELECT id, 1 FROM memory_entries WHERE superseded_by = ?
                 UNION ALL
                 SELECT m.id, older.depth + 1 FROM memory_entries m
                     JOIN older ON m.superseded_by = older.id
             )
             SELECT {COLUMNS} FROM memory_entries
             JOIN older USING (id) ORDER BY older.depth, id DESC"
        );
        query(&self.conn, &sql, &[Box::new(id.0)])
    }

    /// The entries of `scope`, most recently changed first.
    pub fn list_memory(
        &self,
        scope: MemoryScope,
        include_superseded: bool,
    ) -> Result<Vec<MemoryEntry>> {
        let mut parameters = Vec::new();
        let conditions = scope_conditions(scope, include_superseded, &mut parameters);
        let sql = format!(
            "SELECT {COLUMNS} FROM memory_entries WHERE {}
             ORDER BY updated_at DESC, id DESC",
            conditions.join(" AND ")
        );
        query(&self.conn, &sql, &parameters)
    }

    /// The entries of `scope` whose title or body contains every word of `query_text`, as
    /// a substring (case-insensitive for ASCII letters); those whose title has the first word come first, then
    /// the most recently changed. A query without words matches nothing.
    pub fn search_memory(
        &self,
        query_text: &str,
        scope: MemoryScope,
        include_superseded: bool,
        limit: u32,
    ) -> Result<Vec<MemoryEntry>> {
        let words: Vec<&str> = query_text.split_whitespace().collect();
        let Some(first) = words.first() else {
            return Ok(Vec::new());
        };
        let mut parameters = Vec::new();
        let mut conditions = scope_conditions(scope, include_superseded, &mut parameters);
        for word in &words {
            conditions.push("(title || char(10) || body) LIKE ? ESCAPE '\\'".to_owned());
            parameters.push(Box::new(like_pattern(word)));
        }
        parameters.push(Box::new(like_pattern(first)));
        parameters.push(Box::new(limit));
        let sql = format!(
            "SELECT {COLUMNS} FROM memory_entries WHERE {}
             ORDER BY title LIKE ? ESCAPE '\\' DESC, updated_at DESC, id DESC LIMIT ?",
            conditions.join(" AND ")
        );
        query(&self.conn, &sql, &parameters)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;
    use crate::domain::model::{TaskType, Workspace, WorkspaceLocation};
    use crate::domain::path::WindowsPath;
    use crate::storage::{NewTask, NewWorkspace};

    fn storage() -> (Storage, Arc<AtomicI64>) {
        let mut storage = Storage::open_in_memory().unwrap();
        let clock = Arc::new(AtomicI64::new(100));
        let tick = clock.clone();
        storage.set_clock(move || tick.fetch_add(1, Ordering::SeqCst));
        (storage, clock)
    }

    fn workspace(storage: &mut Storage, name: &str) -> Workspace {
        storage
            .create_workspace(&NewWorkspace {
                name: name.to_owned(),
                location: WorkspaceLocation::Local {
                    path: WindowsPath::parse(&format!(r"C:\dev\{name}"))
                        .unwrap()
                        .into(),
                },
                default_base_branch: "main".to_owned(),
            })
            .unwrap()
    }

    fn task(storage: &mut Storage, workspace: &Workspace) -> TaskId {
        storage
            .create_task(&NewTask {
                workspace_id: workspace.id,
                task_type: TaskType::Feat,
                title: "Add login".to_owned(),
                description: String::new(),
                estimate: None,
                due_date: None,
            })
            .unwrap()
            .id
    }

    fn content(kind: MemoryKind, title: &str, body: &str) -> MemoryContent {
        MemoryContent::new(kind, title, body, None).unwrap()
    }

    fn titles(entries: &[MemoryEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.title.as_str()).collect()
    }

    fn of(workspace: &Workspace, with_shared: bool) -> MemoryScope {
        MemoryScope::Workspace {
            workspace: workspace.id,
            with_shared,
        }
    }

    #[test]
    fn workspace_and_shared_entries_are_listed_by_scope() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        let other = workspace(&mut storage, "other");
        let shared = storage
            .add_memory(None, &content(MemoryKind::Rule, "Reply in English", ""))
            .unwrap();
        storage
            .add_memory(
                Some(toodo.id),
                &content(MemoryKind::Decision, "Use SQLite", "Bundled."),
            )
            .unwrap();
        storage
            .add_memory(Some(other.id), &content(MemoryKind::Note, "Other", ""))
            .unwrap();

        assert_eq!(shared.workspace, None);
        assert_eq!((shared.created_at, shared.updated_at), (100, 100));
        assert_eq!(
            titles(&storage.list_memory(of(&toodo, false), false).unwrap()),
            ["Use SQLite"]
        );
        assert_eq!(
            titles(&storage.list_memory(of(&toodo, true), false).unwrap()),
            ["Use SQLite", "Reply in English"]
        );
        assert_eq!(
            titles(&storage.list_memory(MemoryScope::Shared, false).unwrap()),
            ["Reply in English"]
        );
    }

    #[test]
    fn superseding_keeps_the_old_entry_as_history_out_of_the_current_list() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        let first = storage
            .add_memory(
                Some(toodo.id),
                &content(MemoryKind::Decision, "Use make", ""),
            )
            .unwrap();
        let second = storage
            .supersede_memory(
                first.id,
                &content(MemoryKind::Decision, "Use npm scripts", ""),
            )
            .unwrap();
        let third = storage
            .supersede_memory(
                second.id,
                &content(MemoryKind::Rule, "Use npm run", "Always."),
            )
            .unwrap();

        assert_eq!(third.workspace, Some(toodo.id));
        assert_eq!(
            titles(&storage.list_memory(of(&toodo, false), false).unwrap()),
            ["Use npm run"]
        );
        assert_eq!(
            titles(&storage.list_memory(of(&toodo, false), true).unwrap()),
            ["Use npm run", "Use npm scripts", "Use make"]
        );
        assert_eq!(
            titles(&storage.memory_history(third.id).unwrap()),
            ["Use npm scripts", "Use make"]
        );
        let old = storage.get_memory(first.id).unwrap().unwrap();
        assert_eq!(
            (old.title.as_str(), old.updated_at, old.superseded_by),
            ("Use make", first.updated_at, Some(second.id))
        );
        assert!(matches!(
            storage.supersede_memory(first.id, &content(MemoryKind::Note, "Again", "")),
            Err(StorageError::MemorySuperseded { id, by }) if id == first.id && by == second.id
        ));
        assert!(matches!(
            storage.update_memory(second.id, &content(MemoryKind::Note, "Edit", "")),
            Err(StorageError::MemorySuperseded { .. })
        ));
    }

    #[test]
    fn updating_rewrites_a_current_entry_and_moves_its_change_time() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        let id = task(&mut storage, &toodo);
        let added = storage
            .add_memory(Some(toodo.id), &content(MemoryKind::Note, "Typo", ""))
            .unwrap();
        let updated = storage
            .update_memory(
                added.id,
                &MemoryContent::new(MemoryKind::Summary, "Fixed", "Body", Some(id)).unwrap(),
            )
            .unwrap();
        assert_eq!(
            (
                updated.kind,
                updated.title.as_str(),
                updated.body.as_str(),
                updated.task
            ),
            (MemoryKind::Summary, "Fixed", "Body", Some(id))
        );
        assert_eq!(updated.created_at, added.created_at);
        assert!(updated.updated_at > added.updated_at);
        assert!(matches!(
            storage.update_memory(MemoryId(99), &content(MemoryKind::Note, "x", "")),
            Err(StorageError::MemoryNotFound(MemoryId(99)))
        ));
    }

    #[test]
    fn tasks_and_workspaces_of_an_entry_must_exist_and_match() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        let other = workspace(&mut storage, "other");
        let foreign = task(&mut storage, &other);
        let with_task =
            |task| MemoryContent::new(MemoryKind::Note, "Note", "", Some(task)).unwrap();

        assert!(matches!(
            storage.add_memory(Some(toodo.id), &with_task(foreign)),
            Err(StorageError::MemoryTaskMismatch { .. })
        ));
        assert!(matches!(
            storage.add_memory(Some(toodo.id), &with_task(TaskId(99))),
            Err(StorageError::TaskNotFound(TaskId(99)))
        ));
        assert!(matches!(
            storage.add_memory(Some(WorkspaceId(99)), &content(MemoryKind::Note, "x", "")),
            Err(StorageError::WorkspaceNotFound(WorkspaceId(99)))
        ));
        let shared = storage.add_memory(None, &with_task(foreign)).unwrap();
        assert_eq!(shared.task, Some(foreign));
    }

    #[test]
    fn deleting_a_task_unlinks_its_entries_and_deleting_a_workspace_removes_them() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        let id = task(&mut storage, &toodo);
        let entry = storage
            .add_memory(
                Some(toodo.id),
                &MemoryContent::new(MemoryKind::Note, "Note", "", Some(id)).unwrap(),
            )
            .unwrap();
        storage.delete_task(id).unwrap();
        assert_eq!(storage.get_memory(entry.id).unwrap().unwrap().task, None);

        storage.delete_workspace(toodo.id).unwrap();
        assert_eq!(storage.get_memory(entry.id).unwrap(), None);
    }

    #[test]
    fn search_matches_every_word_in_title_or_body_title_matches_first() {
        let (mut storage, _) = storage();
        let toodo = workspace(&mut storage, "toodo");
        storage
            .add_memory(
                Some(toodo.id),
                &content(MemoryKind::Note, "Release flow", "Bump with npm"),
            )
            .unwrap();
        storage
            .add_memory(
                Some(toodo.id),
                &content(MemoryKind::Rule, "Use npm scripts", "not make; 100%_done"),
            )
            .unwrap();
        storage
            .add_memory(None, &content(MemoryKind::Rule, "NPM everywhere", ""))
            .unwrap();
        let old = storage
            .add_memory(Some(toodo.id), &content(MemoryKind::Note, "npm old", ""))
            .unwrap();
        storage
            .supersede_memory(old.id, &content(MemoryKind::Note, "yarn now", ""))
            .unwrap();

        let search = |text: &str, scope, superseded| {
            let found = storage.search_memory(text, scope, superseded, 10).unwrap();
            found.into_iter().map(|e| e.title).collect::<Vec<_>>()
        };
        assert_eq!(
            search("npm", of(&toodo, false), false),
            ["Use npm scripts", "Release flow"]
        );
        assert_eq!(
            search("npm", of(&toodo, true), false),
            ["NPM everywhere", "Use npm scripts", "Release flow"]
        );
        assert_eq!(
            search("npm", of(&toodo, false), true),
            ["npm old", "Use npm scripts", "Release flow"]
        );
        assert_eq!(
            search("BUMP release", of(&toodo, false), false),
            ["Release flow"]
        );
        assert_eq!(search("%_", of(&toodo, false), false), ["Use npm scripts"]);
        assert!(search("  ", of(&toodo, true), true).is_empty());
    }
}
