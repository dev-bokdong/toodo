//! Schema migrations. Each entry upgrades the schema by one version; the current
//! version is stored in `PRAGMA user_version`. Never edit a released migration; append
//! a new one instead.

use rusqlite::{Connection, OptionalExtension};

use super::{Result, StorageError};

const MIGRATIONS: &[&str] = &[
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
}
