//! Schema migrations. Each entry upgrades the schema by one version; the current
//! version is stored in `PRAGMA user_version`. Never edit a released migration; append
//! a new one instead.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{Result, StorageError};
use crate::domain::criteria::{CheckKind, CheckedBy, CriterionState, done_when_items};

pub(super) const MIGRATIONS: &[&str] = &[
    // Version 1: initial schema. AUTOINCREMENT keeps workspace and task IDs from being
    // reused after a delete, because task IDs are part of branch and worktree names.
    r"
    CREATE TABLE workspaces (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
        path TEXT NOT NULL,
        herdr_machine TEXT,
        default_base_branch TEXT NOT NULL,
        CHECK ((kind = 'local') = (herdr_machine IS NULL))
    );

    CREATE TABLE machine_settings (
        kind TEXT NOT NULL CHECK (kind IN ('local', 'ssh')),
        label TEXT NOT NULL DEFAULT '',
        worktree_root TEXT,
        PRIMARY KEY (kind, label),
        CHECK ((kind = 'local') = (label = ''))
    );

    CREATE TABLE tasks (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
        type TEXT NOT NULL
            CHECK (type IN ('feat', 'fix', 'modify', 'refactor', 'docs', 'test', 'chore')),
        title TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL
            CHECK (status IN ('planned', 'ready', 'in_progress', 'done', 'cancelled')),
        branch TEXT,
        worktree_path TEXT,
        herdr_workspace_id TEXT,
        pull_request_url TEXT
    );
    CREATE INDEX tasks_workspace_id ON tasks (workspace_id);

    CREATE TABLE task_dependencies (
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        depends_on_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        PRIMARY KEY (task_id, depends_on_id),
        CHECK (task_id <> depends_on_id)
    ) WITHOUT ROWID;
    CREATE INDEX task_dependencies_depends_on_id ON task_dependencies (depends_on_id);

    CREATE TABLE commit_records (
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        sha TEXT NOT NULL,
        message TEXT NOT NULL,
        author TEXT NOT NULL,
        committed_at INTEGER NOT NULL,
        PRIMARY KEY (task_id, sha)
    );
    ",
    // Version 2: adds the `in_review` task status. SQLite cannot change a CHECK
    // constraint in place, so the tasks table is rebuilt, keeping its AUTOINCREMENT
    // counter so deleted task IDs are still never reused.
    r"
    CREATE TABLE tasks_new (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
        type TEXT NOT NULL
            CHECK (type IN ('feat', 'fix', 'modify', 'refactor', 'docs', 'test', 'chore')),
        title TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL
            CHECK (status IN
                ('planned', 'ready', 'in_progress', 'in_review', 'done', 'cancelled')),
        branch TEXT,
        worktree_path TEXT,
        herdr_workspace_id TEXT,
        pull_request_url TEXT
    );
    INSERT INTO tasks_new (id, workspace_id, type, title, description, status, branch,
        worktree_path, herdr_workspace_id, pull_request_url)
    SELECT id, workspace_id, type, title, description, status, branch, worktree_path,
        herdr_workspace_id, pull_request_url
    FROM tasks;
    DELETE FROM sqlite_sequence WHERE name = 'tasks_new';
    INSERT INTO sqlite_sequence (name, seq)
        SELECT 'tasks_new', seq FROM sqlite_sequence WHERE name = 'tasks';
    DROP TABLE tasks;
    ALTER TABLE tasks_new RENAME TO tasks;
    CREATE INDEX tasks_workspace_id ON tasks (workspace_id);
    ",
    // Version 3: task status history for the timeline, and the merge time of a task's
    // pull request. Tasks created before this version have no history.
    r"
    CREATE TABLE task_events (
        id INTEGER PRIMARY KEY,
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        status TEXT NOT NULL
            CHECK (status IN
                ('planned', 'ready', 'in_progress', 'in_review', 'done', 'cancelled')),
        at INTEGER NOT NULL
    );
    CREATE INDEX task_events_task_id ON task_events (task_id);
    ALTER TABLE tasks ADD COLUMN pull_request_merged_at INTEGER;
    ",
    // Version 4: four task statuses. Whether a task can start follows from its
    // dependencies instead of a `ready` status, and a cancelled task becomes a paused,
    // `planned` one. The first event of a task records its creation, so a leading `ready`
    // event becomes `planned`; the other `ready` events are dropped, and so is every
    // `planned` event that follows another one, which no longer marks a change. The
    // CHECK constraints still allow the old values, which Toodo no longer writes.
    r"
    UPDATE tasks SET status = 'planned' WHERE status IN ('ready', 'cancelled');
    UPDATE task_events SET status = 'planned' WHERE status = 'cancelled';
    UPDATE task_events SET status = 'planned'
        WHERE status = 'ready'
          AND id IN (SELECT min(id) FROM task_events GROUP BY task_id);
    DELETE FROM task_events WHERE status = 'ready';
    DELETE FROM task_events WHERE id IN (
        SELECT id FROM (
            SELECT id, status,
                   lag(status) OVER (PARTITION BY task_id ORDER BY id) AS previous
            FROM task_events
        )
        WHERE status = 'planned' AND previous = 'planned'
    );
    ",
    // Version 5: what reviews need about a completed task beyond its history: the note
    // the user writes about its outcome, and a snapshot of its merged pull request.
    r"
    ALTER TABLE tasks ADD COLUMN completion_note TEXT;
    CREATE TABLE pull_request_snapshots (
        task_id INTEGER PRIMARY KEY REFERENCES tasks (id) ON DELETE CASCADE,
        number INTEGER NOT NULL,
        title TEXT NOT NULL,
        additions INTEGER NOT NULL,
        deletions INTEGER NOT NULL,
        changed_files INTEGER NOT NULL,
        merge_commit_sha TEXT,
        review_count INTEGER NOT NULL,
        reviewer_count INTEGER NOT NULL,
        first_reviewed_at INTEGER
    );
    ",
    // Version 6: the `rnd` task type, and how a `done` task ended: `merged`, or
    // `not_adopted` for work concluded without merging. Every task done before this
    // version was completed by its merge. The tasks table is rebuilt as in version 2 to
    // change its CHECK constraints; statuses that version 4 retired are no longer allowed.
    r"
    CREATE TABLE tasks_new (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
        type TEXT NOT NULL
            CHECK (type IN
                ('feat', 'fix', 'modify', 'refactor', 'docs', 'test', 'chore', 'rnd')),
        title TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL
            CHECK (status IN ('planned', 'in_progress', 'in_review', 'done')),
        resolution TEXT CHECK (resolution IN ('merged', 'not_adopted')),
        branch TEXT,
        worktree_path TEXT,
        herdr_workspace_id TEXT,
        pull_request_url TEXT,
        pull_request_merged_at INTEGER,
        completion_note TEXT,
        CHECK ((status = 'done') = (resolution IS NOT NULL))
    );
    INSERT INTO tasks_new (id, workspace_id, type, title, description, status, resolution,
        branch, worktree_path, herdr_workspace_id, pull_request_url, pull_request_merged_at,
        completion_note)
    SELECT id, workspace_id, type, title, description, status,
        CASE WHEN status = 'done' THEN 'merged' END, branch, worktree_path,
        herdr_workspace_id, pull_request_url, pull_request_merged_at, completion_note
    FROM tasks;
    DELETE FROM sqlite_sequence WHERE name = 'tasks_new';
    INSERT INTO sqlite_sequence (name, seq)
        SELECT 'tasks_new', seq FROM sqlite_sequence WHERE name = 'tasks';
    DROP TABLE tasks;
    ALTER TABLE tasks_new RENAME TO tasks;
    CREATE INDEX tasks_workspace_id ON tasks (workspace_id);
    ",
    // Version 7: the GitHub issue a task works on, whose closing reference Toodo keeps in
    // the task's pull request. Existing tasks have none.
    r"
    ALTER TABLE tasks ADD COLUMN issue_number INTEGER CHECK (issue_number > 0);
    ALTER TABLE tasks ADD COLUMN issue_url TEXT
        CHECK ((issue_url IS NULL) = (issue_number IS NULL));
    ",
    // Version 8: notes that agents leave on a task while working on it, through the MCP
    // server: progress and decisions, or what blocks them. Existing tasks have none.
    r"
    CREATE TABLE task_notes (
        id INTEGER PRIMARY KEY,
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (kind IN ('progress', 'blocker')),
        body TEXT NOT NULL CHECK (body <> ''),
        at INTEGER NOT NULL
    );
    CREATE INDEX task_notes_task_id ON task_notes (task_id);
    ",
    // Version 9: what a task was planned to take, to compare with what it took: an
    // estimate in hours or points and a due day (`YYYY-MM-DD`; `date()` normalizes days a
    // month does not have and returns NULL for other text, so both fail the `IS` check).
    // Existing tasks have neither.
    r"
    ALTER TABLE tasks ADD COLUMN estimate_unit TEXT CHECK (estimate_unit IN ('hours', 'points'));
    ALTER TABLE tasks ADD COLUMN estimate_amount REAL
        CHECK ((estimate_amount IS NULL) = (estimate_unit IS NULL))
        CHECK (estimate_amount > 0);
    ALTER TABLE tasks ADD COLUMN due_date TEXT CHECK (date(due_date) IS due_date);
    ",
    // Version 10: estimates count time only, in hours, days, weeks, or months; points are
    // gone. Estimates in points have no time to compare with, so they are cleared. The
    // estimate columns are recreated to change their CHECK constraints: SQLite drops a
    // column whose own constraints reference another column, but not one referenced by
    // the constraints of another, so the amount goes before the unit.
    r"
    ALTER TABLE tasks RENAME COLUMN estimate_unit TO old_estimate_unit;
    ALTER TABLE tasks RENAME COLUMN estimate_amount TO old_estimate_amount;
    ALTER TABLE tasks ADD COLUMN estimate_unit TEXT
        CHECK (estimate_unit IN ('hours', 'days', 'weeks', 'months'));
    ALTER TABLE tasks ADD COLUMN estimate_amount REAL
        CHECK ((estimate_amount IS NULL) = (estimate_unit IS NULL))
        CHECK (estimate_amount > 0);
    UPDATE tasks SET estimate_unit = old_estimate_unit, estimate_amount = old_estimate_amount
        WHERE old_estimate_unit = 'hours';
    ALTER TABLE tasks DROP COLUMN old_estimate_amount;
    ALTER TABLE tasks DROP COLUMN old_estimate_unit;
    ",
    // Version 11: finding earlier tasks. `task_relations` links tasks that are related
    // without one waiting on the other, in any workspace; a link is stored once, from the
    // lower task ID to the higher one. `task_search` is a full-text index (FTS5 with the
    // trigram tokenizer, so any substring of three or more characters matches, in any
    // language) of what `task_search_source` says about each task: title, description,
    // completion note and agent notes, branch, and pull request title and URL. Its rowid
    // is the task ID, and the triggers below keep it in sync with every write. A later
    // migration that rebuilds one of these tables has to recreate the view and triggers.
    r"
    CREATE TABLE task_relations (
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        related_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        PRIMARY KEY (task_id, related_id),
        CHECK (task_id < related_id)
    ) WITHOUT ROWID;
    CREATE INDEX task_relations_related_id ON task_relations (related_id);

    CREATE VIRTUAL TABLE task_search USING fts5 (
        title, description, notes, branch, pull_request, tokenize = 'trigram'
    );
    CREATE VIEW task_search_source (id, title, description, notes, branch, pull_request) AS
    SELECT t.id, t.title, t.description,
        trim(coalesce(t.completion_note, '') || char(10) || coalesce(
            (SELECT group_concat(n.body, char(10)) FROM task_notes n WHERE n.task_id = t.id),
            ''), char(10)),
        coalesce(t.branch, ''),
        trim(coalesce((SELECT s.title FROM pull_request_snapshots s WHERE s.task_id = t.id), '')
            || ' ' || coalesce(t.pull_request_url, ''))
    FROM tasks t;
    INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
        SELECT * FROM task_search_source;

    CREATE TRIGGER task_search_task_inserted AFTER INSERT ON tasks BEGIN
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = NEW.id;
    END;
    CREATE TRIGGER task_search_task_updated
        AFTER UPDATE OF title, description, completion_note, branch, pull_request_url ON tasks
    BEGIN
        DELETE FROM task_search WHERE rowid = OLD.id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = NEW.id;
    END;
    CREATE TRIGGER task_search_task_deleted AFTER DELETE ON tasks BEGIN
        DELETE FROM task_search WHERE rowid = OLD.id;
    END;
    CREATE TRIGGER task_search_note_inserted AFTER INSERT ON task_notes BEGIN
        DELETE FROM task_search WHERE rowid = NEW.task_id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = NEW.task_id;
    END;
    CREATE TRIGGER task_search_note_deleted AFTER DELETE ON task_notes BEGIN
        DELETE FROM task_search WHERE rowid = OLD.task_id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = OLD.task_id;
    END;
    CREATE TRIGGER task_search_pull_request_inserted AFTER INSERT ON pull_request_snapshots
    BEGIN
        DELETE FROM task_search WHERE rowid = NEW.task_id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = NEW.task_id;
    END;
    CREATE TRIGGER task_search_pull_request_updated AFTER UPDATE ON pull_request_snapshots
    BEGIN
        DELETE FROM task_search WHERE rowid = OLD.task_id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = OLD.task_id;
    END;
    CREATE TRIGGER task_search_pull_request_deleted AFTER DELETE ON pull_request_snapshots
    BEGIN
        DELETE FROM task_search WHERE rowid = OLD.task_id;
        INSERT INTO task_search (rowid, title, description, notes, branch, pull_request)
            SELECT * FROM task_search_source WHERE id = OLD.task_id;
    END;
    ",
    // Version 12: whether agents may merge their own pull request into the base branch.
    // Existing workspaces keep today's behavior: agents never merge.
    "
    ALTER TABLE workspaces ADD COLUMN agent_merge TEXT NOT NULL DEFAULT 'never'
        CHECK (agent_merge IN ('never', 'after_checks'));
    ",
    // Version 13: a task done as merged through the pull request of another task, e.g.
    // one pull request that fixed several tasks, records that pull request and its task.
    "
    ALTER TABLE tasks ADD COLUMN merged_with_pull_request_url TEXT
        CHECK (merged_with_pull_request_url IS NULL OR resolution = 'merged');
    ALTER TABLE tasks ADD COLUMN merged_with_task_id INTEGER
        REFERENCES tasks (id) ON DELETE SET NULL
        CHECK (merged_with_task_id IS NULL OR merged_with_pull_request_url IS NOT NULL);
    ",
    // Version 14: project memory. Entries belong to one workspace, or to none, which
    // makes them shared by every workspace. Replacing an entry keeps it, pointing at its
    // replacement through `superseded_by`; current entries have none.
    "
    CREATE TABLE memory_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id INTEGER REFERENCES workspaces (id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (kind IN ('decision', 'rule', 'note', 'summary')),
        title TEXT NOT NULL CHECK (trim(title) <> ''),
        body TEXT NOT NULL DEFAULT '',
        task_id INTEGER REFERENCES tasks (id) ON DELETE SET NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        superseded_by INTEGER REFERENCES memory_entries (id) ON DELETE SET NULL,
        CHECK (superseded_by IS NULL OR superseded_by <> id)
    );
    CREATE INDEX memory_entries_workspace_id ON memory_entries (workspace_id);
    CREATE INDEX memory_entries_superseded_by ON memory_entries (superseded_by);
    ",
    // Version 15: devices that use toodo-server, each with its own access token, stored
    // only as a SHA-256 hash. Revoked devices are kept, so their id stays unique.
    "
    CREATE TABLE devices (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        label TEXT NOT NULL CHECK (trim(label) <> ''),
        token_hash TEXT NOT NULL UNIQUE CHECK (length(token_hash) = 64),
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        revoked_at INTEGER
    );
    ",
    // Version 16: completion criteria. A task is done once every criterion is met; Toodo
    // checks every kind but `manual` on its own. `target` holds the URL or command to
    // check, or the commit a `branch_merged_locally` branch started from. Existing tasks
    // get theirs from the data step [`import_criteria`].
    "
    CREATE TABLE task_criteria (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        task_id INTEGER NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        text TEXT NOT NULL CHECK (trim(text) <> ''),
        kind TEXT NOT NULL CHECK (kind IN ('manual', 'pr_merged', 'pr_open_ci_green',
            'branch_merged_locally', 'url', 'command')),
        target TEXT,
        state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'met', 'failed')),
        evidence TEXT,
        checked_by TEXT CHECK (checked_by IN ('user', 'agent', 'toodo')),
        checked_at INTEGER,
        CHECK (CASE kind
            WHEN 'url' THEN target IS NOT NULL
            WHEN 'command' THEN target IS NOT NULL
            WHEN 'branch_merged_locally' THEN 1
            ELSE target IS NULL END)
    );
    CREATE INDEX task_criteria_task_id ON task_criteria (task_id, position);
    ",
    // Version 17: the Web Push subscription of each device, with the kinds of
    // notification it wants. A device has at most one (its browser's), and an endpoint
    // belongs to one device.
    "
    CREATE TABLE push_subscriptions (
        device_id INTEGER PRIMARY KEY REFERENCES devices (id) ON DELETE CASCADE,
        endpoint TEXT NOT NULL UNIQUE CHECK (endpoint LIKE 'https://%'),
        p256dh BLOB NOT NULL CHECK (length(p256dh) = 65),
        auth BLOB NOT NULL CHECK (length(auth) = 16),
        agent INTEGER NOT NULL DEFAULT 1 CHECK (agent IN (0, 1)),
        pull_requests INTEGER NOT NULL DEFAULT 1 CHECK (pull_requests IN (0, 1)),
        completion INTEGER NOT NULL DEFAULT 1 CHECK (completion IN (0, 1)),
        created_at INTEGER NOT NULL
    );
    ",
    // Version 18: the conversation with the lead in the chat panel. A client names each
    // message it sends, so sending it again does not repeat it.
    "
    CREATE TABLE chat_messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        author TEXT NOT NULL CHECK (author IN ('user', 'lead')),
        body TEXT NOT NULL CHECK (trim(body) <> ''),
        client_id TEXT UNIQUE CHECK (client_id IS NULL OR author = 'user'),
        at INTEGER NOT NULL
    );
    ",
    // Version 19: when a person's message reached the lead's pane, and when the lead
    // picked it up. Earlier messages were all typed into the pane before they were
    // stored; those the lead answered afterwards count as picked up.
    "
    ALTER TABLE chat_messages ADD COLUMN delivered_at INTEGER
        CHECK (delivered_at IS NULL OR author = 'user');
    ALTER TABLE chat_messages ADD COLUMN seen_at INTEGER
        CHECK (seen_at IS NULL OR delivered_at IS NOT NULL);
    UPDATE chat_messages SET delivered_at = at WHERE author = 'user';
    UPDATE chat_messages SET seen_at = delivered_at
        WHERE author = 'user'
          AND EXISTS (SELECT 1 FROM chat_messages AS later
                      WHERE later.author = 'lead' AND later.id > chat_messages.id);
    ",
];

/// Data steps that run right after the SQL of the migration to the given version, for
/// changes SQL alone cannot make.
type DataStep = fn(&Transaction<'_>) -> Result<()>;

const DATA_STEPS: &[(i64, DataStep)] = &[(16, import_criteria)];

/// Gives every existing task the criteria it had so far: its pull request is merged
/// (met for tasks done as merged, through their own or another task's pull request),
/// followed by the `- [ ]` and `- [x]` items of the `Done when` section of its
/// description as manual criteria, checked items met. The description stays as it is.
fn import_criteria(tx: &Transaction<'_>) -> Result<()> {
    let tasks = {
        let mut statement = tx.prepare(
            "SELECT t.id, t.description, t.status = 'done' AND t.resolution = 'merged',
                 coalesce(t.merged_with_pull_request_url, t.pull_request_url),
                 (SELECT max(e.at) FROM task_events e
                  WHERE e.task_id = t.id AND e.status = 'done')
             FROM tasks t ORDER BY t.id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut insert = tx.prepare(
        "INSERT INTO task_criteria (task_id, position, text, kind, state, evidence,
             checked_by, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for (id, description, merged, pull_request_url, done_at) in tasks {
        let (state, evidence, checked_by, checked_at) = if merged {
            (
                CriterionState::Met,
                pull_request_url,
                Some(CheckedBy::Toodo.as_str()),
                done_at,
            )
        } else {
            (CriterionState::Pending, None, None, None)
        };
        insert.execute(params![
            id,
            0,
            "Pull request merged into the base branch",
            CheckKind::PullRequestMerged.as_str(),
            state.as_str(),
            evidence,
            checked_by,
            checked_at
        ])?;
        for (position, (text, checked)) in (1_i64..).zip(done_when_items(&description)) {
            let (state, evidence) = if checked {
                (CriterionState::Met, Some("Checked in the task description"))
            } else {
                (CriterionState::Pending, None)
            };
            insert.execute(params![
                id,
                position,
                text,
                CheckKind::Manual.as_str(),
                state.as_str(),
                evidence,
                None::<&str>,
                None::<i64>
            ])?;
        }
    }
    Ok(())
}

pub(super) const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

pub(super) fn schema_version(conn: &Connection) -> Result<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Migrates the schema to [`LATEST_VERSION`] in one transaction. Foreign key enforcement
/// is turned off first, because rebuilding a referenced table would otherwise cascade
/// deletes into its children; the result is checked with `PRAGMA foreign_key_check`
/// instead. The caller turns enforcement back on.
pub(super) fn migrate(conn: &mut Connection) -> Result<()> {
    let version = schema_version(conn)?;
    if version > LATEST_VERSION {
        return Err(StorageError::UnsupportedSchemaVersion {
            found: version,
            supported: LATEST_VERSION,
        });
    }
    conn.pragma_update(None, "foreign_keys", false)?;
    let tx = conn.transaction()?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        tx.execute_batch(sql)?;
        let version = index as i64 + 1;
        for (_, step) in DATA_STEPS.iter().filter(|(after, _)| *after == version) {
            step(&tx)?;
        }
        tx.pragma_update(None, "user_version", version)?;
    }
    let violation: Option<String> = tx
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()?;
    if let Some(table) = violation {
        return Err(StorageError::InvalidData(format!(
            "migration left a dangling foreign key in table {table}"
        )));
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_2_keeps_tasks_their_references_and_the_id_counter() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status, pull_request_url)
                VALUES (1, 'feat', 'A', 'done', 'https://github.com/acme/toodo/pull/1'),
                       (1, 'fix', 'B', 'ready', NULL),
                       (1, 'chore', 'C', 'ready', NULL);
            INSERT INTO task_dependencies (task_id, depends_on_id) VALUES (2, 1);
            INSERT INTO commit_records (task_id, position, sha, message, author, committed_at)
                VALUES (1, 0, 'aaa', 'Add A', 'Ada', 10);
            DELETE FROM tasks WHERE id = 3;
            ",
        )
        .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(
            (
                count("tasks"),
                count("task_dependencies"),
                count("commit_records")
            ),
            (2, 1, 1)
        );
        conn.execute(
            "INSERT INTO tasks (workspace_id, type, title, status) VALUES (1, 'feat', 'D', 'in_review')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 4);
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(
            (count("task_dependencies"), count("commit_records")),
            (0, 0)
        );
    }

    #[test]
    fn version_3_adds_an_empty_history_and_no_merge_time_to_existing_tasks() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..2] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status, pull_request_url)
                VALUES (1, 'feat', 'A', 'done', 'https://github.com/acme/toodo/pull/1');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let merged_at: Option<i64> = conn
            .query_row(
                "SELECT pull_request_merged_at FROM tasks WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(merged_at, None);
        let events = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM task_events", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(events(&conn), 0);
        assert!(
            conn.execute(
                "INSERT INTO task_events (task_id, status, at) VALUES (1, 'merged', 1)",
                []
            )
            .is_err()
        );
        conn.execute(
            "INSERT INTO task_events (task_id, status, at) VALUES (1, 'done', 1)",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(events(&conn), 0);
    }

    #[test]
    fn version_4_turns_ready_and_cancelled_into_planned_and_drops_readiness_events() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..3] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status)
                VALUES (1, 'feat', 'Ready', 'ready'),
                       (1, 'feat', 'Waiting', 'planned'),
                       (1, 'feat', 'Cancelled', 'cancelled'),
                       (1, 'feat', 'Done', 'done');
            INSERT INTO task_events (task_id, status, at)
                VALUES (1, 'ready', 10), (1, 'planned', 20), (1, 'ready', 30),
                       (2, 'planned', 10), (2, 'ready', 20), (2, 'planned', 30),
                       (3, 'ready', 10), (3, 'in_progress', 20), (3, 'cancelled', 30),
                       (3, 'planned', 40), (3, 'ready', 50),
                       (4, 'ready', 10), (4, 'in_progress', 20), (4, 'in_review', 30),
                       (4, 'done', 40);
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        let statuses: Vec<String> = conn
            .prepare("SELECT status FROM tasks ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(statuses, ["planned", "planned", "planned", "done"]);
        let history = |task: i64| -> Vec<(String, i64)> {
            conn.prepare("SELECT status, at FROM task_events WHERE task_id = ?1 ORDER BY id")
                .unwrap()
                .query_map([task], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let events = |raw: &[(&str, i64)]| -> Vec<(String, i64)> {
            raw.iter()
                .map(|&(status, at)| (status.to_owned(), at))
                .collect()
        };
        assert_eq!(history(1), events(&[("planned", 10)]));
        assert_eq!(history(2), events(&[("planned", 10)]));
        assert_eq!(
            history(3),
            events(&[("planned", 10), ("in_progress", 20), ("planned", 30)])
        );
        assert_eq!(
            history(4),
            events(&[
                ("planned", 10),
                ("in_progress", 20),
                ("in_review", 30),
                ("done", 40)
            ])
        );
    }

    #[test]
    fn version_5_adds_no_note_and_no_snapshot_to_existing_tasks() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status, pull_request_url)
                VALUES (1, 'feat', 'A', 'done', 'https://github.com/acme/toodo/pull/1');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let note: Option<String> = conn
            .query_row(
                "SELECT completion_note FROM tasks WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(note, None);
        let snapshots = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM pull_request_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(snapshots(&conn), 0);
        conn.execute(
            "INSERT INTO pull_request_snapshots (task_id, number, title, additions, deletions,
                 changed_files, review_count, reviewer_count)
             VALUES (1, 1, 'A', 1, 0, 1, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(snapshots(&conn), 0);
    }

    #[test]
    fn version_7_links_no_issue_to_existing_tasks() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..6] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 6).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status) VALUES (1, 'fix', 'A', 'planned');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let issue: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT issue_number, issue_url FROM tasks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(issue, (None, None));
        conn.execute(
            "UPDATE tasks SET issue_number = 12, issue_url = 'https://github.com/acme/toodo/issues/12'
             WHERE id = 1",
            [],
        )
        .unwrap();
        for invalid in [
            "UPDATE tasks SET issue_url = NULL WHERE id = 1",
            "UPDATE tasks SET issue_number = NULL WHERE id = 1",
            "UPDATE tasks SET issue_number = 0 WHERE id = 1",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
    }

    #[test]
    fn version_8_adds_no_notes_and_drops_them_with_their_task() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..7] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status) VALUES (1, 'fix', 'A', 'planned');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let notes = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM task_notes", [], |row| row.get(0))
                .unwrap()
        };
        assert_eq!(notes(&conn), 0);
        for invalid in [
            "INSERT INTO task_notes (task_id, kind, body, at) VALUES (1, 'idea', 'x', 1)",
            "INSERT INTO task_notes (task_id, kind, body, at) VALUES (1, 'progress', '', 1)",
            "INSERT INTO task_notes (task_id, kind, body, at) VALUES (9, 'progress', 'x', 1)",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
        conn.execute(
            "INSERT INTO task_notes (task_id, kind, body, at) VALUES (1, 'blocker', 'No access', 1)",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(notes(&conn), 0);
    }

    #[test]
    fn version_9_plans_nothing_for_existing_tasks_and_checks_new_plans() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..8] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status) VALUES (1, 'fix', 'A', 'planned');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let plan: (Option<String>, Option<f64>, Option<String>) = conn
            .query_row(
                "SELECT estimate_unit, estimate_amount, due_date FROM tasks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(plan, (None, None, None));
        conn.execute(
            "UPDATE tasks SET estimate_unit = 'hours', estimate_amount = 1.5,
                 due_date = '2024-02-29' WHERE id = 1",
            [],
        )
        .unwrap();
        for invalid in [
            "UPDATE tasks SET estimate_unit = 'minutes' WHERE id = 1",
            "UPDATE tasks SET estimate_unit = NULL WHERE id = 1",
            "UPDATE tasks SET estimate_amount = NULL WHERE id = 1",
            "UPDATE tasks SET estimate_amount = 0 WHERE id = 1",
            "UPDATE tasks SET due_date = '2026-02-29' WHERE id = 1",
            "UPDATE tasks SET due_date = '2026-10-15 12:00' WHERE id = 1",
            "UPDATE tasks SET due_date = 'tomorrow' WHERE id = 1",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
    }

    #[test]
    fn version_10_keeps_hour_estimates_clears_points_and_allows_time_units() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..9] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 9).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status, estimate_unit,
                    estimate_amount, due_date)
                VALUES (1, 'fix', 'A', 'planned', 'hours', 1.5, '2026-10-20'),
                       (1, 'feat', 'B', 'planned', 'points', 3, NULL),
                       (1, 'docs', 'C', 'planned', NULL, NULL, NULL);
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let mut statement = conn
            .prepare("SELECT estimate_unit, estimate_amount, due_date FROM tasks ORDER BY id")
            .unwrap();
        let plans: Vec<(Option<String>, Option<f64>, Option<String>)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            plans,
            [
                (
                    Some("hours".to_owned()),
                    Some(1.5),
                    Some("2026-10-20".to_owned())
                ),
                (None, None, None),
                (None, None, None),
            ]
        );
        for unit in ["days", "weeks", "months", "hours"] {
            conn.execute("UPDATE tasks SET estimate_unit = ?1 WHERE id = 1", [unit])
                .unwrap();
        }
        for invalid in [
            "UPDATE tasks SET estimate_unit = 'points' WHERE id = 1",
            "UPDATE tasks SET estimate_unit = NULL WHERE id = 1",
            "UPDATE tasks SET estimate_amount = 0 WHERE id = 1",
            "UPDATE tasks SET estimate_unit = 'days' WHERE id = 2",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
    }

    #[test]
    fn version_11_indexes_existing_tasks_for_search_and_relates_none() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..10] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 10).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, description, status, resolution,
                    branch, pull_request_url, completion_note)
                VALUES (1, 'feat', 'Ship the server', 'Deploy to the home box', 'done',
                        'merged', 'feat/ship-server-1',
                        'https://github.com/acme/toodo/pull/7', 'Chose systemd'),
                       (1, 'fix', 'Other', '', 'planned', NULL, NULL, NULL, NULL);
            INSERT INTO task_notes (task_id, kind, body, at)
                VALUES (2, 'progress', 'Picked rusqlite over sqlx', 5);
            INSERT INTO pull_request_snapshots (task_id, number, title, additions, deletions,
                    changed_files, review_count, reviewer_count)
                VALUES (1, 7, 'Add toodo-server', 1, 0, 1, 0, 0);
            ",
        )
        .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let matches = |query: &str| -> Vec<i64> {
            conn.prepare("SELECT rowid FROM task_search WHERE task_search MATCH ?1 ORDER BY rowid")
                .unwrap()
                .query_map([query], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        for (query, found) in [
            ("\"home box\"", vec![1]),
            ("systemd", vec![1]),
            ("\"toodo-server\"", vec![1]),
            ("\"pull/7\"", vec![1]),
            ("\"ship-server\"", vec![1]),
            ("rusqlite", vec![2]),
            // A substring of any word: "Ship the server" and "Other".
            ("\"the\"", vec![1, 2]),
        ] {
            assert_eq!(matches(query), found, "{query}");
        }
        let relations: i64 = conn
            .query_row("SELECT count(*) FROM task_relations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(relations, 0);
        for invalid in [
            "INSERT INTO task_relations (task_id, related_id) VALUES (2, 1)",
            "INSERT INTO task_relations (task_id, related_id) VALUES (1, 1)",
            "INSERT INTO task_relations (task_id, related_id) VALUES (1, 9)",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
    }

    #[test]
    fn version_6_marks_done_tasks_merged_keeps_their_data_and_allows_rnd() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, status, branch, pull_request_url,
                    pull_request_merged_at, completion_note)
                VALUES (1, 'feat', 'A', 'done', 'feat/a-1',
                        'https://github.com/acme/toodo/pull/1', 40, 'Shipped'),
                       (1, 'fix', 'B', 'in_progress', 'fix/b-2', NULL, NULL, NULL),
                       (1, 'chore', 'C', 'planned', NULL, NULL, NULL, NULL);
            INSERT INTO task_dependencies (task_id, depends_on_id) VALUES (2, 1);
            INSERT INTO task_events (task_id, status, at) VALUES (1, 'done', 40);
            INSERT INTO commit_records (task_id, position, sha, message, author, committed_at)
                VALUES (1, 0, 'aaa', 'Add A', 'Ada', 10);
            INSERT INTO pull_request_snapshots (task_id, number, title, additions, deletions,
                    changed_files, review_count, reviewer_count)
                VALUES (1, 1, 'A', 1, 0, 1, 0, 0);
            DELETE FROM tasks WHERE id = 3;
            ",
        )
        .unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        // Status, resolution, completion note, and merge time.
        type Row = (String, Option<String>, Option<String>, Option<i64>);
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT status, resolution, completion_note, pull_request_merged_at
                 FROM tasks ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (
                    "done".to_owned(),
                    Some("merged".to_owned()),
                    Some("Shipped".to_owned()),
                    Some(40)
                ),
                ("in_progress".to_owned(), None, None, None),
            ]
        );
        conn.execute(
            "INSERT INTO tasks (workspace_id, type, title, status) VALUES (1, 'rnd', 'D', 'planned')",
            [],
        )
        .unwrap();
        // Deleted task IDs are still never reused.
        assert_eq!(conn.last_insert_rowid(), 4);
        for invalid in [
            "UPDATE tasks SET status = 'done' WHERE id = 2",
            "UPDATE tasks SET resolution = 'merged' WHERE id = 2",
            "UPDATE tasks SET resolution = 'abandoned' WHERE id = 1",
            "UPDATE tasks SET status = 'cancelled' WHERE id = 2",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
        let count = |table: &str| -> i64 {
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(
            [
                "task_dependencies",
                "task_events",
                "commit_records",
                "pull_request_snapshots"
            ]
            .map(count),
            [1, 1, 1, 1]
        );
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(
            [
                "task_dependencies",
                "task_events",
                "commit_records",
                "pull_request_snapshots"
            ]
            .map(count),
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn version_12_lets_agents_of_existing_workspaces_never_merge() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..11] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 11).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let agent_merge = |conn: &Connection| -> String {
            conn.query_row(
                "SELECT agent_merge FROM workspaces WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(agent_merge(&conn), "never");
        assert!(
            conn.execute("UPDATE workspaces SET agent_merge = 'always'", [])
                .is_err()
        );
        conn.execute("UPDATE workspaces SET agent_merge = 'after_checks'", [])
            .unwrap();
        assert_eq!(agent_merge(&conn), "after_checks");
    }

    #[test]
    fn version_13_keeps_tasks_and_links_only_merged_ones_to_another_pull_request() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..12] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 12).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, description, status, resolution,
                    pull_request_url)
                VALUES (1, 'fix', 'Batch', '', 'done', 'merged',
                    'https://github.com/acme/toodo/pull/1'),
                (1, 'fix', 'Extra', '', 'done', 'not_adopted', NULL);
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let merged_with = |conn: &Connection| -> (Option<String>, Option<i64>) {
            conn.query_row(
                "SELECT merged_with_pull_request_url, merged_with_task_id FROM tasks WHERE id = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(merged_with(&conn), (None, None));
        for invalid in [
            // Work that was not adopted did not ship with a pull request.
            "UPDATE tasks SET merged_with_pull_request_url = 'https://github.com/acme/toodo/pull/1'
             WHERE id = 2",
            "UPDATE tasks SET resolution = 'merged', merged_with_task_id = 1 WHERE id = 2",
            "UPDATE tasks SET resolution = 'merged', merged_with_pull_request_url = 'x',
                merged_with_task_id = 9 WHERE id = 2",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
        conn.execute(
            "UPDATE tasks SET resolution = 'merged',
                 merged_with_pull_request_url = 'https://github.com/acme/toodo/pull/1',
                 merged_with_task_id = 1
             WHERE id = 2",
            [],
        )
        .unwrap();
        // Deleting the task that owns the pull request keeps the link to the pull request.
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert_eq!(
            merged_with(&conn),
            (
                Some("https://github.com/acme/toodo/pull/1".to_owned()),
                None
            )
        );
    }

    #[test]
    fn version_16_turns_done_when_items_into_criteria_after_the_pull_request() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..15] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 15).unwrap();
        conn.execute_batch(
            r"
            INSERT INTO workspaces (name, kind, path, default_base_branch)
                VALUES ('toodo', 'local', 'C:\dev\toodo', 'main');
            INSERT INTO tasks (workspace_id, type, title, description, status, resolution,
                    pull_request_url)
                VALUES (1, 'feat', 'Open', '## Purpose
- [ ] not this

## Done when
- [ ] Tests pass
- [x] Docs updated
', 'in_progress', NULL, NULL),
                (1, 'fix', 'Shipped', 'No sections', 'done', 'merged',
                    'https://github.com/acme/toodo/pull/7'),
                (1, 'rnd', 'Dropped', '', 'done', 'not_adopted', NULL);
            INSERT INTO task_events (task_id, status, at) VALUES (2, 'done', 50);
            ",
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        type Row = (String, String, String, Option<String>, Option<i64>);
        let criteria = |task: i64| -> Vec<Row> {
            let mut statement = conn
                .prepare(
                    "SELECT text, kind, state, evidence, checked_at FROM task_criteria
                     WHERE task_id = ?1 ORDER BY position",
                )
                .unwrap();
            statement
                .query_map([task], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let merged = "Pull request merged into the base branch".to_owned();
        assert_eq!(
            criteria(1),
            vec![
                (
                    merged.clone(),
                    "pr_merged".into(),
                    "pending".into(),
                    None,
                    None
                ),
                (
                    "Tests pass".into(),
                    "manual".into(),
                    "pending".into(),
                    None,
                    None
                ),
                (
                    "Docs updated".into(),
                    "manual".into(),
                    "met".into(),
                    Some("Checked in the task description".into()),
                    None
                ),
            ]
        );
        assert_eq!(
            criteria(2),
            vec![(
                merged.clone(),
                "pr_merged".into(),
                "met".into(),
                Some("https://github.com/acme/toodo/pull/7".into()),
                Some(50)
            )]
        );
        assert_eq!(
            criteria(3),
            vec![(merged, "pr_merged".into(), "pending".into(), None, None)]
        );
        for invalid in [
            "INSERT INTO task_criteria (task_id, position, text, kind) VALUES (1, 9, 'x', 'url')",
            "INSERT INTO task_criteria (task_id, position, text, kind, target)
                 VALUES (1, 9, 'x', 'manual', 'y')",
            "INSERT INTO task_criteria (task_id, position, text, kind) VALUES (1, 9, ' ', 'manual')",
        ] {
            assert!(conn.execute(invalid, []).is_err(), "{invalid}");
        }
        conn.execute("DELETE FROM tasks WHERE id = 1", []).unwrap();
        assert!(criteria(1).is_empty());
    }

    #[test]
    fn version_17_keeps_devices_without_subscriptions_and_drops_them_with_the_device() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..16] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 16).unwrap();
        conn.execute(
            "INSERT INTO devices (label, token_hash, created_at, last_seen_at)
             VALUES ('Phone', ?1, 1, 1)",
            ["0".repeat(64)],
        )
        .unwrap();

        migrate(&mut conn).unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let count = |conn: &Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM push_subscriptions", [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(count(&conn), 0);
        let insert = |endpoint: &str, p256dh: usize, auth: usize| {
            conn.execute(
                "INSERT INTO push_subscriptions (device_id, endpoint, p256dh, auth, created_at)
                 VALUES (1, ?1, ?2, ?3, 2)",
                params![endpoint, vec![4u8; p256dh], vec![1u8; auth]],
            )
        };
        assert!(insert("http://push.example/1", 65, 16).is_err());
        assert!(insert("https://push.example/1", 64, 16).is_err());
        assert!(insert("https://push.example/1", 65, 15).is_err());
        insert("https://push.example/1", 65, 16).unwrap();
        let settings: (i64, i64, i64) = conn
            .query_row(
                "SELECT agent, pull_requests, completion FROM push_subscriptions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(settings, (1, 1, 1));
        conn.execute("DELETE FROM devices WHERE id = 1", [])
            .unwrap();
        assert_eq!(count(&conn), 0);
    }

    #[test]
    fn version_18_starts_an_empty_chat_that_keeps_one_message_per_client_id() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..17] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 17).unwrap();

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let insert = |author: &str, body: &str, client_id: Option<&str>| {
            conn.execute(
                "INSERT INTO chat_messages (author, body, client_id, at) VALUES (?1, ?2, ?3, 1)",
                params![author, body, client_id],
            )
        };
        assert!(insert("user", "Hi", Some("c1")).is_ok());
        assert!(insert("user", "Hi", Some("c1")).is_err());
        assert!(insert("lead", "Hello", None).is_ok());
        assert!(insert("lead", "Hello", Some("c2")).is_err());
        assert!(insert("user", " ", None).is_err());
        assert!(insert("dori", "Hi", None).is_err());
    }

    #[test]
    fn version_19_counts_earlier_messages_delivered_and_answered_ones_seen() {
        let mut conn = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..18] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 18).unwrap();
        for (author, at) in [("user", 10), ("lead", 20), ("user", 30)] {
            conn.execute(
                "INSERT INTO chat_messages (author, body, at) VALUES (?1, 'Hi', ?2)",
                params![author, at],
            )
            .unwrap();
        }

        migrate(&mut conn).unwrap();

        assert_eq!(schema_version(&conn).unwrap(), LATEST_VERSION);
        let rows: Vec<(Option<i64>, Option<i64>)> = conn
            .prepare("SELECT delivered_at, seen_at FROM chat_messages ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, [(Some(10), Some(10)), (None, None), (Some(30), None)]);
        let set = |sql: &str| conn.execute(sql, []);
        assert!(set("UPDATE chat_messages SET delivered_at = 1 WHERE author = 'lead'").is_err());
        assert!(set("UPDATE chat_messages SET delivered_at = NULL WHERE id = 1").is_err());
    }
}
