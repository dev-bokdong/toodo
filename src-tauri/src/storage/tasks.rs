//! Tasks, their dependency edges, and status changes.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Result, Storage, StorageError, invalid_data};
use crate::domain::dag;
use crate::domain::model::{
    CommitRecord, HostPath, PullRequestSnapshot, Task, TaskId, TaskType, WorkspaceId,
};
use crate::domain::naming::TaskCheckout;
use crate::domain::path::{PosixPath, WindowsPath};
use crate::domain::status::TaskStatus;
use crate::domain::timeline::TaskEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    pub description: String,
}

const TASK_TYPES: [TaskType; 7] = [
    TaskType::Feat,
    TaskType::Fix,
    TaskType::Modify,
    TaskType::Refactor,
    TaskType::Docs,
    TaskType::Test,
    TaskType::Chore,
];

const TASK_STATUSES: [TaskStatus; 4] = [
    TaskStatus::Planned,
    TaskStatus::InProgress,
    TaskStatus::InReview,
    TaskStatus::Done,
];

fn parse_task_type(raw: &str) -> Result<TaskType> {
    TASK_TYPES
        .into_iter()
        .find(|task_type| task_type.as_str() == raw)
        .ok_or_else(|| StorageError::InvalidData(format!("unknown task type {raw:?}")))
}

fn parse_task_status(raw: &str) -> Result<TaskStatus> {
    TASK_STATUSES
        .into_iter()
        .find(|status| status.as_str() == raw)
        .ok_or_else(|| StorageError::InvalidData(format!("unknown task status {raw:?}")))
}

const TASK_SELECT: &str = "SELECT t.id, t.workspace_id, t.type, t.title, t.description, t.status,
        t.branch, t.worktree_path, t.herdr_workspace_id, t.pull_request_url,
        t.pull_request_merged_at, w.kind, t.completion_note, s.number, s.title, s.additions,
        s.deletions, s.changed_files, s.merge_commit_sha, s.review_count, s.reviewer_count,
        s.first_reviewed_at
    FROM tasks t JOIN workspaces w ON w.id = t.workspace_id
        LEFT JOIN pull_request_snapshots s ON s.task_id = t.id";

struct TaskRow {
    id: i64,
    workspace_id: i64,
    task_type: String,
    title: String,
    description: String,
    status: String,
    branch: Option<String>,
    worktree_path: Option<String>,
    herdr_workspace_id: Option<String>,
    pull_request_url: Option<String>,
    pull_request_merged_at: Option<i64>,
    workspace_kind: String,
    completion_note: Option<String>,
    pull_request: Option<PullRequestSnapshot>,
}

impl TaskRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            workspace_id: row.get(1)?,
            task_type: row.get(2)?,
            title: row.get(3)?,
            description: row.get(4)?,
            status: row.get(5)?,
            branch: row.get(6)?,
            worktree_path: row.get(7)?,
            herdr_workspace_id: row.get(8)?,
            pull_request_url: row.get(9)?,
            pull_request_merged_at: row.get(10)?,
            workspace_kind: row.get(11)?,
            completion_note: row.get(12)?,
            pull_request: match row.get::<_, Option<i64>>(13)? {
                None => None,
                Some(number) => Some(PullRequestSnapshot {
                    number,
                    title: row.get(14)?,
                    additions: row.get(15)?,
                    deletions: row.get(16)?,
                    changed_files: row.get(17)?,
                    merge_commit_sha: row.get(18)?,
                    review_count: row.get(19)?,
                    reviewer_count: row.get(20)?,
                    first_reviewed_at: row.get(21)?,
                }),
            },
        })
    }

    fn into_task(self, depends_on: BTreeSet<TaskId>, timeline: Vec<TaskEvent>) -> Result<Task> {
        let worktree_path = match (self.worktree_path, self.workspace_kind.as_str()) {
            (None, _) => None,
            (Some(raw), "local") => Some(HostPath::Windows(
                WindowsPath::parse(&raw).map_err(invalid_data)?,
            )),
            (Some(raw), _) => Some(HostPath::Posix(
                PosixPath::parse(&raw).map_err(invalid_data)?,
            )),
        };
        Ok(Task {
            id: TaskId(self.id),
            workspace_id: WorkspaceId(self.workspace_id),
            task_type: parse_task_type(&self.task_type)?,
            title: self.title,
            description: self.description,
            status: parse_task_status(&self.status)?,
            depends_on,
            branch: self.branch,
            worktree_path,
            herdr_workspace_id: self.herdr_workspace_id,
            pull_request_url: self.pull_request_url,
            pull_request_merged_at: self.pull_request_merged_at,
            pull_request: self.pull_request,
            completion_note: self.completion_note,
            timeline,
        })
    }
}

/// Tasks matching `filter`, a condition on the `tasks` table aliased `t` that takes the
/// single parameter `?1`.
fn query_tasks(conn: &Connection, filter: &str, parameter: i64) -> Result<Vec<Task>> {
    let mut statement =
        conn.prepare_cached(&format!("{TASK_SELECT} WHERE {filter} ORDER BY t.id"))?;
    let rows = statement
        .query_map([parameter], TaskRow::read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut statement = conn.prepare_cached(&format!(
        "SELECT d.task_id, d.depends_on_id
         FROM task_dependencies d JOIN tasks t ON t.id = d.task_id
         WHERE {filter}"
    ))?;
    let mut depends_on: HashMap<i64, BTreeSet<TaskId>> = HashMap::new();
    for edge in statement.query_map([parameter], |row| {
        Ok((row.get::<_, i64>(0)?, TaskId(row.get(1)?)))
    })? {
        let (task, dependency) = edge?;
        depends_on.entry(task).or_default().insert(dependency);
    }

    let mut statement = conn.prepare_cached(&format!(
        "SELECT e.task_id, e.status, e.at
         FROM task_events e JOIN tasks t ON t.id = e.task_id
         WHERE {filter} ORDER BY e.id"
    ))?;
    let mut timelines: HashMap<i64, Vec<TaskEvent>> = HashMap::new();
    for event in statement.query_map([parameter], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })? {
        let (task, status, at) = event?;
        timelines.entry(task).or_default().push(TaskEvent {
            status: parse_task_status(&status)?,
            at,
        });
    }

    rows.into_iter()
        .map(|row| {
            let dependencies = depends_on.remove(&row.id).unwrap_or_default();
            let timeline = timelines.remove(&row.id).unwrap_or_default();
            row.into_task(dependencies, timeline)
        })
        .collect()
}

/// Appends a status change of `task` at `at` to its status history.
fn record_event(conn: &Connection, task: TaskId, status: TaskStatus, at: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO task_events (task_id, status, at) VALUES (?1, ?2, ?3)",
        params![task.0, status.as_str(), at],
    )?;
    Ok(())
}

fn workspace_tasks(conn: &Connection, workspace: WorkspaceId) -> Result<Vec<Task>> {
    query_tasks(conn, "t.workspace_id = ?1", workspace.0)
}

/// Every task of every workspace: the dependency DAG spans workspaces.
fn all_tasks(conn: &Connection) -> Result<Vec<Task>> {
    query_tasks(conn, "?1 = 1", 1)
}

fn find_task(conn: &Connection, id: TaskId) -> Result<Option<Task>> {
    Ok(query_tasks(conn, "t.id = ?1", id.0)?.pop())
}

fn task_workspace(conn: &Connection, id: TaskId) -> Result<Option<WorkspaceId>> {
    Ok(conn
        .query_row(
            "SELECT workspace_id FROM tasks WHERE id = ?1",
            [id.0],
            |row| row.get(0).map(WorkspaceId),
        )
        .optional()?)
}

/// The stored form of a worktree path, which must belong to the machine kind of the
/// task's workspace.
fn stored_worktree_path<'a>(
    task: TaskId,
    workspace_kind: &str,
    path: Option<&'a HostPath>,
) -> Result<Option<&'a str>> {
    match (path, workspace_kind) {
        (None, _) => Ok(None),
        (Some(HostPath::Windows(path)), "local") => Ok(Some(path.as_str())),
        (Some(HostPath::Posix(path)), "ssh") => Ok(Some(path.as_str())),
        (Some(_), _) => Err(StorageError::WorktreePathMismatch(task)),
    }
}

/// Rejects starting `task` while one of its dependencies, in any workspace, is not done.
fn ensure_ready(conn: &Connection, task: TaskId) -> Result<()> {
    if dag::is_ready(&all_tasks(conn)?, task)? {
        Ok(())
    } else {
        Err(StorageError::DependenciesNotDone(task))
    }
}

impl Storage {
    /// Creates a `planned` task without dependencies and records its creation as the
    /// first event of its history.
    pub fn create_task(&mut self, new: &NewTask) -> Result<Task> {
        let status = TaskStatus::Planned;
        let at = self.now();
        let tx = self.conn.transaction()?;
        let inserted = tx.execute(
            "INSERT INTO tasks (workspace_id, type, title, description, status)
             SELECT id, ?2, ?3, ?4, ?5 FROM workspaces WHERE id = ?1",
            params![
                new.workspace_id.0,
                new.task_type.as_str(),
                new.title,
                new.description,
                status.as_str()
            ],
        )?;
        if inserted == 0 {
            return Err(StorageError::WorkspaceNotFound(new.workspace_id));
        }
        let id = TaskId(tx.last_insert_rowid());
        record_event(&tx, id, status, at)?;
        tx.commit()?;
        Ok(Task {
            id,
            workspace_id: new.workspace_id,
            task_type: new.task_type,
            title: new.title.clone(),
            description: new.description.clone(),
            status,
            depends_on: BTreeSet::new(),
            branch: None,
            worktree_path: None,
            herdr_workspace_id: None,
            pull_request_url: None,
            pull_request_merged_at: None,
            pull_request: None,
            completion_note: None,
            timeline: vec![TaskEvent { status, at }],
        })
    }

    pub fn get_task(&self, id: TaskId) -> Result<Option<Task>> {
        find_task(&self.conn, id)
    }

    /// Tasks of `workspace` ordered by ID, each with its dependencies.
    pub fn list_tasks(&self, workspace: WorkspaceId) -> Result<Vec<Task>> {
        workspace_tasks(&self.conn, workspace)
    }

    /// Tasks of every workspace ordered by ID, each with its dependencies.
    pub fn list_all_tasks(&self) -> Result<Vec<Task>> {
        all_tasks(&self.conn)
    }

    /// Saves the editable fields of `task`: type, title, description, branch, worktree
    /// path, herdr workspace, and pull request URL. Status, dependencies, and workspace
    /// are changed through their own methods and are ignored here.
    pub fn update_task(&mut self, task: &Task) -> Result<()> {
        let tx = self.conn.transaction()?;
        let workspace_kind: String = tx
            .query_row(
                "SELECT w.kind FROM tasks t JOIN workspaces w ON w.id = t.workspace_id
                 WHERE t.id = ?1",
                [task.id.0],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::TaskNotFound(task.id))?;
        let worktree_path =
            stored_worktree_path(task.id, &workspace_kind, task.worktree_path.as_ref())?;
        tx.execute(
            "UPDATE tasks
             SET type = ?2, title = ?3, description = ?4, branch = ?5, worktree_path = ?6,
                 herdr_workspace_id = ?7, pull_request_url = ?8
             WHERE id = ?1",
            params![
                task.id.0,
                task.task_type.as_str(),
                task.title,
                task.description,
                task.branch,
                worktree_path,
                task.herdr_workspace_id,
                task.pull_request_url
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rejects starting a task while one of its dependencies is not done; check this
    /// before preparing its worktree.
    pub fn ensure_ready(&self, id: TaskId) -> Result<()> {
        ensure_ready(&self.conn, id)
    }

    /// Records the worktree of a `planned` task whose dependencies are done and moves it
    /// to `in_progress` in one transaction, so a task is never in progress without its
    /// worktree.
    pub fn start_task(
        &mut self,
        id: TaskId,
        checkout: &TaskCheckout,
        herdr_workspace_id: &str,
    ) -> Result<Task> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        let status = task.status.transition_to(TaskStatus::InProgress)?;
        ensure_ready(&tx, id)?;
        let workspace_kind: String = tx.query_row(
            "SELECT kind FROM workspaces WHERE id = ?1",
            [task.workspace_id.0],
            |row| row.get(0),
        )?;
        let worktree_path = stored_worktree_path(id, &workspace_kind, Some(&checkout.path))?;
        tx.execute(
            "UPDATE tasks
             SET status = ?2, branch = ?3, worktree_path = ?4, herdr_workspace_id = ?5
             WHERE id = ?1",
            params![
                id.0,
                status.as_str(),
                checkout.branch,
                worktree_path,
                herdr_workspace_id
            ],
        )?;
        record_event(&tx, id, status, at)?;
        let started = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(started)
    }

    /// Moves an `in_review` task to `done`, stores the merge time and the snapshot of its
    /// pull request, and forgets its herdr workspace, which was closed, in one
    /// transaction. Non-empty `commits` (those of the merged pull request) replace its
    /// commit records. The worktree path stays: the checkout is kept on disk until its
    /// worktree is removed. Dependents whose other dependencies are done can then start.
    pub fn complete_task(
        &mut self,
        id: TaskId,
        merged_at: Option<i64>,
        pull_request: Option<&PullRequestSnapshot>,
        commits: &[CommitRecord],
    ) -> Result<Task> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        let status = task.status.transition_to(TaskStatus::Done)?;
        tx.execute(
            "UPDATE tasks
             SET status = ?2, herdr_workspace_id = NULL, pull_request_merged_at = ?3
             WHERE id = ?1",
            params![id.0, status.as_str(), merged_at],
        )?;
        if let Some(snapshot) = pull_request {
            tx.execute(
                "INSERT OR REPLACE INTO pull_request_snapshots (task_id, number, title,
                     additions, deletions, changed_files, merge_commit_sha, review_count,
                     reviewer_count, first_reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id.0,
                    snapshot.number,
                    snapshot.title,
                    snapshot.additions,
                    snapshot.deletions,
                    snapshot.changed_files,
                    snapshot.merge_commit_sha,
                    snapshot.review_count,
                    snapshot.reviewer_count,
                    snapshot.first_reviewed_at
                ],
            )?;
        }
        if !commits.is_empty() {
            super::commits::write_commit_records(&tx, id, commits)?;
        }
        record_event(&tx, id, status, at)?;
        let done = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(done)
    }

    /// Stores what the user wrote about the outcome of a task; `None` clears it. Whether
    /// the task may have a note is up to the caller.
    pub fn set_completion_note(&mut self, id: TaskId, note: Option<&str>) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            "UPDATE tasks SET completion_note = ?2 WHERE id = ?1",
            params![id.0, note],
        )?;
        if updated == 0 {
            return Err(StorageError::TaskNotFound(id));
        }
        let noted = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(noted)
    }

    /// Forgets the worktree and herdr workspace of a task after its worktree was removed.
    /// The branch, status, commit records, and pull request stay.
    pub fn clear_task_worktree(&mut self, id: TaskId) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let updated = tx.execute(
            "UPDATE tasks SET worktree_path = NULL, herdr_workspace_id = NULL WHERE id = ?1",
            [id.0],
        )?;
        if updated == 0 {
            return Err(StorageError::TaskNotFound(id));
        }
        let cleared = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(cleared)
    }

    /// Records the worktree a task got back after its previous one was removed, and the
    /// herdr workspace it is open in. Status, branch, and history are unchanged.
    pub fn restore_task_worktree(
        &mut self,
        id: TaskId,
        path: &HostPath,
        herdr_workspace_id: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let workspace_kind: String = tx
            .query_row(
                "SELECT w.kind FROM tasks t JOIN workspaces w ON w.id = t.workspace_id
                 WHERE t.id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(StorageError::TaskNotFound(id))?;
        let worktree_path = stored_worktree_path(id, &workspace_kind, Some(path))?;
        tx.execute(
            "UPDATE tasks SET worktree_path = ?2, herdr_workspace_id = ?3 WHERE id = ?1",
            params![id.0, worktree_path, herdr_workspace_id],
        )?;
        let restored = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(restored)
    }

    /// Records the herdr workspace a task's worktree was opened again in.
    pub fn set_task_herdr_workspace(&mut self, id: TaskId, herdr_workspace_id: &str) -> Result<()> {
        let updated = self.conn.execute(
            "UPDATE tasks SET herdr_workspace_id = ?2 WHERE id = ?1",
            params![id.0, herdr_workspace_id],
        )?;
        if updated == 0 {
            return Err(StorageError::TaskNotFound(id));
        }
        Ok(())
    }

    /// Records the pull request of an `in_progress` or `in_review` task in one
    /// transaction: replaces its commit records, stores the pull request URL and its
    /// merge time (`None` until merged), and moves it to (or keeps it in) `in_review`.
    /// Dependents stay waiting until it is `done`.
    pub fn open_pull_request(
        &mut self,
        id: TaskId,
        records: &[CommitRecord],
        pull_request_url: &str,
        merged_at: Option<i64>,
    ) -> Result<Task> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        let moved = task.status != TaskStatus::InReview;
        if moved {
            task.status.transition_to(TaskStatus::InReview)?;
        }
        super::commits::write_commit_records(&tx, id, records)?;
        tx.execute(
            "UPDATE tasks SET status = ?2, pull_request_url = ?3, pull_request_merged_at = ?4
             WHERE id = ?1",
            params![
                id.0,
                TaskStatus::InReview.as_str(),
                pull_request_url,
                merged_at
            ],
        )?;
        if moved {
            record_event(&tx, id, TaskStatus::InReview, at)?;
        }
        let in_review = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(in_review)
    }

    /// Moves a task to `status` if its current status allows it, recording the change;
    /// pausing is a move back to `planned`. Starting requires all dependencies to be
    /// done. The pull request checks that guard `in_review` and `done` are up to the
    /// caller.
    pub fn set_task_status(&mut self, id: TaskId, status: TaskStatus) -> Result<Task> {
        let at = self.now();
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        task.status.transition_to(status)?;
        if status == TaskStatus::InProgress {
            ensure_ready(&tx, id)?;
        }
        tx.execute(
            "UPDATE tasks SET status = ?2 WHERE id = ?1",
            params![id.0, status.as_str()],
        )?;
        record_event(&tx, id, status, at)?;
        let changed = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(changed)
    }

    /// Deletes a task with its dependency edges, commit records, and status history.
    pub fn delete_task(&mut self, id: TaskId) -> Result<()> {
        let deleted = self
            .conn
            .execute("DELETE FROM tasks WHERE id = ?1", [id.0])?;
        if deleted == 0 {
            return Err(StorageError::TaskNotFound(id));
        }
        Ok(())
    }

    /// Makes `task` depend on `depends_on`, which may belong to another workspace. Rejects
    /// unknown tasks, self dependencies, and edges that would close a cycle anywhere in the
    /// DAG of all workspaces, writing nothing.
    pub fn add_dependency(&mut self, task: TaskId, depends_on: TaskId) -> Result<()> {
        let tx = self.conn.transaction()?;
        dag::validate_dependency(&all_tasks(&tx)?, task, depends_on)?;
        tx.execute(
            "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on_id) VALUES (?1, ?2)",
            [task.0, depends_on.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Removes the edge `task -> depends_on` if present.
    pub fn remove_dependency(&mut self, task: TaskId, depends_on: TaskId) -> Result<()> {
        let tx = self.conn.transaction()?;
        task_workspace(&tx, task)?.ok_or(StorageError::TaskNotFound(task))?;
        tx.execute(
            "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on_id = ?2",
            [task.0, depends_on.0],
        )?;
        tx.commit()?;
        Ok(())
    }
}
