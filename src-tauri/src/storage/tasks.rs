//! Tasks, their dependency edges, and status changes.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{Result, Storage, StorageError, invalid_data};
use crate::domain::dag::{self, DependencyError, StatusChange};
use crate::domain::model::{CommitRecord, HostPath, Task, TaskId, TaskType, WorkspaceId};
use crate::domain::naming::TaskCheckout;
use crate::domain::path::{PosixPath, WindowsPath};
use crate::domain::status::TaskStatus;

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

const TASK_STATUSES: [TaskStatus; 6] = [
    TaskStatus::Planned,
    TaskStatus::Ready,
    TaskStatus::InProgress,
    TaskStatus::InReview,
    TaskStatus::Done,
    TaskStatus::Cancelled,
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
        t.branch, t.worktree_path, t.herdr_workspace_id, t.pull_request_url, w.kind
    FROM tasks t JOIN workspaces w ON w.id = t.workspace_id";

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
    workspace_kind: String,
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
            workspace_kind: row.get(10)?,
        })
    }

    fn into_task(self, depends_on: BTreeSet<TaskId>) -> Result<Task> {
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

    rows.into_iter()
        .map(|row| {
            let dependencies = depends_on.remove(&row.id).unwrap_or_default();
            row.into_task(dependencies)
        })
        .collect()
}

fn workspace_tasks(conn: &Connection, workspace: WorkspaceId) -> Result<Vec<Task>> {
    query_tasks(conn, "t.workspace_id = ?1", workspace.0)
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

/// Applies [`dag::readiness_changes`] to the tasks of `workspace` and returns them.
fn sync_readiness(conn: &Connection, workspace: WorkspaceId) -> Result<Vec<StatusChange>> {
    let changes = dag::readiness_changes(&workspace_tasks(conn, workspace)?)?;
    for change in &changes {
        conn.execute(
            "UPDATE tasks SET status = ?2 WHERE id = ?1",
            params![change.task.0, change.to.as_str()],
        )?;
    }
    Ok(changes)
}

impl Storage {
    /// Creates a task without dependencies, so it starts out `ready`.
    pub fn create_task(&mut self, new: &NewTask) -> Result<Task> {
        let status = TaskStatus::Ready;
        let inserted = self.conn.execute(
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
        Ok(Task {
            id: TaskId(self.conn.last_insert_rowid()),
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
        })
    }

    pub fn get_task(&self, id: TaskId) -> Result<Option<Task>> {
        find_task(&self.conn, id)
    }

    /// Tasks of `workspace` ordered by ID, each with its dependencies.
    pub fn list_tasks(&self, workspace: WorkspaceId) -> Result<Vec<Task>> {
        workspace_tasks(&self.conn, workspace)
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

    /// Records the worktree of a `ready` task and moves it to `in_progress` in one
    /// transaction, so a task is never in progress without its worktree.
    pub fn start_task(
        &mut self,
        id: TaskId,
        checkout: &TaskCheckout,
        herdr_workspace_id: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        let status = task.status.transition_to(TaskStatus::InProgress)?;
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
        let started = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(started)
    }

    /// Moves an `in_review` task to `done` and forgets its herdr workspace, which was
    /// closed, in one transaction. The worktree path stays: the checkout is kept on disk
    /// until its worktree is removed. Dependents may become `ready`.
    pub fn complete_task(&mut self, id: TaskId) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        let status = task.status.transition_to(TaskStatus::Done)?;
        tx.execute(
            "UPDATE tasks SET status = ?2, herdr_workspace_id = NULL WHERE id = ?1",
            params![id.0, status.as_str()],
        )?;
        sync_readiness(&tx, task.workspace_id)?;
        let done = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(done)
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

    /// Records the open pull request of an `in_progress` or `in_review` task in one
    /// transaction: replaces its commit records, stores the pull request URL, and moves
    /// it to (or keeps it in) `in_review`. Dependents stay waiting until it is `done`.
    pub fn open_pull_request(
        &mut self,
        id: TaskId,
        records: &[CommitRecord],
        pull_request_url: &str,
    ) -> Result<Task> {
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        if task.status != TaskStatus::InReview {
            task.status.transition_to(TaskStatus::InReview)?;
        }
        super::commits::write_commit_records(&tx, id, records)?;
        tx.execute(
            "UPDATE tasks SET status = ?2, pull_request_url = ?3 WHERE id = ?1",
            params![id.0, TaskStatus::InReview.as_str(), pull_request_url],
        )?;
        let in_review = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.commit()?;
        Ok(in_review)
    }

    /// Moves a task to `in_progress`, `in_review`, `done`, or `cancelled` if its current
    /// status allows it, and returns the readiness changes this caused in other tasks.
    pub fn set_task_status(&mut self, id: TaskId, status: TaskStatus) -> Result<Vec<StatusChange>> {
        if matches!(status, TaskStatus::Planned | TaskStatus::Ready) {
            return Err(StorageError::DerivedStatus(status));
        }
        let tx = self.conn.transaction()?;
        let task = find_task(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        task.status.transition_to(status)?;
        tx.execute(
            "UPDATE tasks SET status = ?2 WHERE id = ?1",
            params![id.0, status.as_str()],
        )?;
        let changes = sync_readiness(&tx, task.workspace_id)?;
        tx.commit()?;
        Ok(changes)
    }

    /// Deletes a task with its dependency edges and commit records, and returns the
    /// readiness changes this caused in its dependents.
    pub fn delete_task(&mut self, id: TaskId) -> Result<Vec<StatusChange>> {
        let tx = self.conn.transaction()?;
        let workspace = task_workspace(&tx, id)?.ok_or(StorageError::TaskNotFound(id))?;
        tx.execute("DELETE FROM tasks WHERE id = ?1", [id.0])?;
        let changes = sync_readiness(&tx, workspace)?;
        tx.commit()?;
        Ok(changes)
    }

    /// Makes `task` depend on `depends_on`. Rejects unknown tasks, self dependencies,
    /// edges across workspaces, and edges that would close a cycle, writing nothing.
    /// Returns the readiness changes the new edge caused.
    pub fn add_dependency(
        &mut self,
        task: TaskId,
        depends_on: TaskId,
    ) -> Result<Vec<StatusChange>> {
        let tx = self.conn.transaction()?;
        let workspace = if task == depends_on {
            None
        } else {
            task_workspace(&tx, task)?
        };
        let mut tasks = match workspace {
            Some(workspace) => workspace_tasks(&tx, workspace)?,
            None => Vec::new(),
        };
        if !tasks.iter().any(|candidate| candidate.id == depends_on) {
            tasks.extend(find_task(&tx, depends_on)?);
        }
        dag::validate_dependency(&tasks, task, depends_on)?;
        let workspace = workspace.ok_or(DependencyError::UnknownTask(task))?;
        tx.execute(
            "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on_id) VALUES (?1, ?2)",
            [task.0, depends_on.0],
        )?;
        let changes = sync_readiness(&tx, workspace)?;
        tx.commit()?;
        Ok(changes)
    }

    /// Removes the edge `task -> depends_on` if present and returns the readiness changes
    /// this caused.
    pub fn remove_dependency(
        &mut self,
        task: TaskId,
        depends_on: TaskId,
    ) -> Result<Vec<StatusChange>> {
        let tx = self.conn.transaction()?;
        let workspace = task_workspace(&tx, task)?.ok_or(StorageError::TaskNotFound(task))?;
        tx.execute(
            "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on_id = ?2",
            [task.0, depends_on.0],
        )?;
        let changes = sync_readiness(&tx, workspace)?;
        tx.commit()?;
        Ok(changes)
    }
}
