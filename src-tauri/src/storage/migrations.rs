//! Schema migrations. Each entry upgrades the schema by one version; the current
//! version is stored in `PRAGMA user_version`. Never edit a released migration; append
//! a new one instead.

use rusqlite::{Connection, OptionalExtension};

use super::{Result, StorageError};

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
];

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
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
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
}
