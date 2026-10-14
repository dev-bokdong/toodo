//! Workspaces and per-machine settings.

use rusqlite::{OptionalExtension, Row, params};

use super::{Result, Storage, StorageError, invalid_data};
use crate::domain::model::{
    Machine, MachineLabel, MachineSettings, TaskId, Workspace, WorkspaceId, WorkspaceLocation,
};
use crate::domain::path::{PosixPath, WindowsPath};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWorkspace {
    pub name: String,
    pub location: WorkspaceLocation,
    pub default_base_branch: String,
}

const WORKSPACE_COLUMNS: &str = "id, name, kind, path, herdr_machine, default_base_branch";

/// `(kind, path, herdr_machine)` columns of a workspace location.
fn location_columns(location: &WorkspaceLocation) -> (&'static str, &str, Option<&str>) {
    match location {
        WorkspaceLocation::Local { path } => ("local", path.as_str(), None),
        WorkspaceLocation::Ssh { machine, path } => ("ssh", path.as_str(), Some(machine.as_str())),
    }
}

/// `(kind, label)` key of a machine in `machine_settings`.
fn machine_key(machine: &Machine) -> (&'static str, &str) {
    match machine {
        Machine::Local => ("local", ""),
        Machine::Ssh(label) => ("ssh", label.as_str()),
    }
}

struct WorkspaceRow {
    id: i64,
    name: String,
    kind: String,
    path: String,
    herdr_machine: Option<String>,
    default_base_branch: String,
}

impl WorkspaceRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            path: row.get(3)?,
            herdr_machine: row.get(4)?,
            default_base_branch: row.get(5)?,
        })
    }

    fn into_workspace(self) -> Result<Workspace> {
        let location = match (self.kind.as_str(), self.herdr_machine) {
            ("local", None) => WorkspaceLocation::Local {
                path: WindowsPath::parse(&self.path).map_err(invalid_data)?,
            },
            ("ssh", Some(machine)) => WorkspaceLocation::Ssh {
                machine: MachineLabel::parse(&machine).map_err(invalid_data)?,
                path: PosixPath::parse(&self.path).map_err(invalid_data)?,
            },
            (kind, _) => {
                return Err(StorageError::InvalidData(format!(
                    "workspace {} has an inconsistent location of kind {kind:?}",
                    self.id
                )));
            }
        };
        Ok(Workspace {
            id: WorkspaceId(self.id),
            name: self.name,
            location,
            default_base_branch: self.default_base_branch,
        })
    }
}

impl Storage {
    pub fn create_workspace(&mut self, new: &NewWorkspace) -> Result<Workspace> {
        let (kind, path, herdr_machine) = location_columns(&new.location);
        self.conn.execute(
            "INSERT INTO workspaces (name, kind, path, herdr_machine, default_base_branch)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![new.name, kind, path, herdr_machine, new.default_base_branch],
        )?;
        Ok(Workspace {
            id: WorkspaceId(self.conn.last_insert_rowid()),
            name: new.name.clone(),
            location: new.location.clone(),
            default_base_branch: new.default_base_branch.clone(),
        })
    }

    pub fn get_workspace(&self, id: WorkspaceId) -> Result<Option<Workspace>> {
        self.conn
            .query_row(
                &format!("SELECT {WORKSPACE_COLUMNS} FROM workspaces WHERE id = ?1"),
                [id.0],
                WorkspaceRow::read,
            )
            .optional()?
            .map(WorkspaceRow::into_workspace)
            .transpose()
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {WORKSPACE_COLUMNS} FROM workspaces ORDER BY id"
        ))?;
        let rows = statement
            .query_map([], WorkspaceRow::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(WorkspaceRow::into_workspace).collect()
    }

    pub fn update_workspace(&mut self, workspace: &Workspace) -> Result<()> {
        let (kind, path, herdr_machine) = location_columns(&workspace.location);
        let changed = self.conn.execute(
            "UPDATE workspaces
             SET name = ?2, kind = ?3, path = ?4, herdr_machine = ?5, default_base_branch = ?6
             WHERE id = ?1",
            params![
                workspace.id.0,
                workspace.name,
                kind,
                path,
                herdr_machine,
                workspace.default_base_branch
            ],
        )?;
        if changed == 0 {
            return Err(StorageError::WorkspaceNotFound(workspace.id));
        }
        Ok(())
    }

    /// Deletes a workspace together with its tasks, dependencies, and commit records.
    /// Rejected while a `planned` task of another workspace depends on one of its tasks,
    /// which would otherwise silently become ready to start; dependencies of tasks being
    /// worked on or done are dropped with the workspace.
    pub fn delete_workspace(&mut self, id: WorkspaceId) -> Result<()> {
        let tx = self.conn.transaction()?;
        let dependent: Option<(i64, i64)> = tx
            .query_row(
                "SELECT d.task_id, d.depends_on_id
                 FROM task_dependencies d
                 JOIN tasks t ON t.id = d.task_id
                 JOIN tasks p ON p.id = d.depends_on_id
                 WHERE p.workspace_id = ?1 AND t.workspace_id <> ?1
                   AND t.status = 'planned'
                 ORDER BY d.task_id, d.depends_on_id
                 LIMIT 1",
                [id.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((task, depends_on)) = dependent {
            return Err(StorageError::WorkspaceHasDependents {
                workspace: id,
                task: TaskId(task),
                depends_on: TaskId(depends_on),
            });
        }
        let changed = tx.execute("DELETE FROM workspaces WHERE id = ?1", [id.0])?;
        if changed == 0 {
            return Err(StorageError::WorkspaceNotFound(id));
        }
        tx.commit()?;
        Ok(())
    }

    /// Settings of `machine`; a machine without saved settings has no worktree root.
    pub fn get_machine_settings(&self, machine: &Machine) -> Result<MachineSettings> {
        let (kind, label) = machine_key(machine);
        let worktree_root: Option<String> = self
            .conn
            .query_row(
                "SELECT worktree_root FROM machine_settings WHERE kind = ?1 AND label = ?2",
                [kind, label],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(match machine {
            Machine::Local => MachineSettings::Local {
                worktree_root: worktree_root
                    .map(|root| WindowsPath::parse(&root))
                    .transpose()
                    .map_err(invalid_data)?,
            },
            Machine::Ssh(label) => MachineSettings::Ssh {
                machine: label.clone(),
                worktree_root: worktree_root
                    .map(|root| PosixPath::parse(&root))
                    .transpose()
                    .map_err(invalid_data)?,
            },
        })
    }

    pub fn save_machine_settings(&mut self, settings: &MachineSettings) -> Result<()> {
        let machine = settings.machine();
        let (kind, label) = machine_key(&machine);
        let worktree_root = match settings {
            MachineSettings::Local { worktree_root } => worktree_root.as_ref().map(|r| r.as_str()),
            MachineSettings::Ssh { worktree_root, .. } => {
                worktree_root.as_ref().map(|r| r.as_str())
            }
        };
        self.conn.execute(
            "INSERT INTO machine_settings (kind, label, worktree_root) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind, label) DO UPDATE SET worktree_root = excluded.worktree_root",
            params![kind, label, worktree_root],
        )?;
        Ok(())
    }
}
