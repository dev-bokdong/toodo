//! Serializable shapes of the domain model exchanged with the frontend. Inputs are
//! validated when they are converted into domain types.

use serde::{Deserialize, Serialize};
use specta::Type;
use specta_typescript::Number;

use super::{CommandError, CommandResult};
use crate::domain::{dag, model, path, status};
use crate::storage;

// Database IDs and timestamps stay far below 2^53, so they are exported as `number`
// instead of the `bigint` specta requires for `i64` by default.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct WorkspaceId(#[specta(type = Number)] pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(transparent)]
pub struct TaskId(#[specta(type = Number)] pub i64);

impl From<WorkspaceId> for model::WorkspaceId {
    fn from(id: WorkspaceId) -> Self {
        Self(id.0)
    }
}

impl From<model::WorkspaceId> for WorkspaceId {
    fn from(id: model::WorkspaceId) -> Self {
        Self(id.0)
    }
}

impl From<TaskId> for model::TaskId {
    fn from(id: TaskId) -> Self {
        Self(id.0)
    }
}

impl From<model::TaskId> for TaskId {
    fn from(id: model::TaskId) -> Self {
        Self(id.0)
    }
}

fn required(field: &str, value: String) -> CommandResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CommandError::invalid_input(format!(
            "{field} must not be empty"
        )));
    }
    Ok(trimmed.to_owned())
}

fn machine_label(raw: &str) -> CommandResult<model::MachineLabel> {
    model::MachineLabel::parse(raw).map_err(|error| CommandError::invalid_input(error.to_string()))
}

fn windows_path(raw: &str) -> CommandResult<path::WindowsPath> {
    path::WindowsPath::parse(raw).map_err(|error| CommandError::invalid_input(error.to_string()))
}

fn posix_path(raw: &str) -> CommandResult<path::PosixPath> {
    path::PosixPath::parse(raw).map_err(|error| CommandError::invalid_input(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkspaceLocation {
    /// Windows path of a repository on this machine.
    Local { path: String },
    /// POSIX path of a repository on a saved herdr SSH machine.
    Ssh { machine: String, path: String },
}

impl From<&model::WorkspaceLocation> for WorkspaceLocation {
    fn from(location: &model::WorkspaceLocation) -> Self {
        match location {
            model::WorkspaceLocation::Local { path } => Self::Local {
                path: path.as_str().to_owned(),
            },
            model::WorkspaceLocation::Ssh { machine, path } => Self::Ssh {
                machine: machine.as_str().to_owned(),
                path: path.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<WorkspaceLocation> for model::WorkspaceLocation {
    type Error = CommandError;

    fn try_from(location: WorkspaceLocation) -> CommandResult<Self> {
        Ok(match location {
            WorkspaceLocation::Local { path } => Self::Local {
                path: windows_path(&path)?,
            },
            WorkspaceLocation::Ssh { machine, path } => Self::Ssh {
                machine: machine_label(&machine)?,
                path: posix_path(&path)?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub location: WorkspaceLocation,
    pub default_base_branch: String,
}

impl From<&model::Workspace> for Workspace {
    fn from(workspace: &model::Workspace) -> Self {
        Self {
            id: workspace.id.into(),
            name: workspace.name.clone(),
            location: (&workspace.location).into(),
            default_base_branch: workspace.default_base_branch.clone(),
        }
    }
}

impl TryFrom<Workspace> for model::Workspace {
    type Error = CommandError;

    fn try_from(workspace: Workspace) -> CommandResult<Self> {
        Ok(Self {
            id: workspace.id.into(),
            name: required("workspace name", workspace.name)?,
            location: workspace.location.try_into()?,
            default_base_branch: required("default base branch", workspace.default_base_branch)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NewWorkspace {
    pub name: String,
    pub location: WorkspaceLocation,
    pub default_base_branch: String,
}

impl TryFrom<NewWorkspace> for storage::NewWorkspace {
    type Error = CommandError;

    fn try_from(new: NewWorkspace) -> CommandResult<Self> {
        Ok(Self {
            name: required("workspace name", new.name)?,
            location: new.location.try_into()?,
            default_base_branch: required("default base branch", new.default_base_branch)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Machine {
    Local,
    Ssh { label: String },
}

impl TryFrom<Machine> for model::Machine {
    type Error = CommandError;

    fn try_from(machine: Machine) -> CommandResult<Self> {
        Ok(match machine {
            Machine::Local => Self::Local,
            Machine::Ssh { label } => Self::Ssh(machine_label(&label)?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MachineSettings {
    pub machine: Machine,
    /// Directory that holds task worktrees; required before a task on this machine can
    /// be started.
    pub worktree_root: Option<String>,
}

impl From<&model::MachineSettings> for MachineSettings {
    fn from(settings: &model::MachineSettings) -> Self {
        match settings {
            model::MachineSettings::Local { worktree_root } => Self {
                machine: Machine::Local,
                worktree_root: worktree_root.as_ref().map(|root| root.as_str().to_owned()),
            },
            model::MachineSettings::Ssh {
                machine,
                worktree_root,
            } => Self {
                machine: Machine::Ssh {
                    label: machine.as_str().to_owned(),
                },
                worktree_root: worktree_root.as_ref().map(|root| root.as_str().to_owned()),
            },
        }
    }
}

impl TryFrom<MachineSettings> for model::MachineSettings {
    type Error = CommandError;

    fn try_from(settings: MachineSettings) -> CommandResult<Self> {
        let root = settings.worktree_root.as_deref();
        Ok(match settings.machine.try_into()? {
            model::Machine::Local => Self::Local {
                worktree_root: root.map(windows_path).transpose()?,
            },
            model::Machine::Ssh(machine) => Self::Ssh {
                machine,
                worktree_root: root.map(posix_path).transpose()?,
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Feat,
    Fix,
    Modify,
    Refactor,
    Docs,
    Test,
    Chore,
}

impl From<model::TaskType> for TaskType {
    fn from(task_type: model::TaskType) -> Self {
        match task_type {
            model::TaskType::Feat => Self::Feat,
            model::TaskType::Fix => Self::Fix,
            model::TaskType::Modify => Self::Modify,
            model::TaskType::Refactor => Self::Refactor,
            model::TaskType::Docs => Self::Docs,
            model::TaskType::Test => Self::Test,
            model::TaskType::Chore => Self::Chore,
        }
    }
}

impl From<TaskType> for model::TaskType {
    fn from(task_type: TaskType) -> Self {
        match task_type {
            TaskType::Feat => Self::Feat,
            TaskType::Fix => Self::Fix,
            TaskType::Modify => Self::Modify,
            TaskType::Refactor => Self::Refactor,
            TaskType::Docs => Self::Docs,
            TaskType::Test => Self::Test,
            TaskType::Chore => Self::Chore,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Planned,
    Ready,
    InProgress,
    Done,
    Cancelled,
}

impl From<status::TaskStatus> for TaskStatus {
    fn from(status: status::TaskStatus) -> Self {
        match status {
            status::TaskStatus::Planned => Self::Planned,
            status::TaskStatus::Ready => Self::Ready,
            status::TaskStatus::InProgress => Self::InProgress,
            status::TaskStatus::Done => Self::Done,
            status::TaskStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<TaskStatus> for status::TaskStatus {
    fn from(status: TaskStatus) -> Self {
        match status {
            TaskStatus::Planned => Self::Planned,
            TaskStatus::Ready => Self::Ready,
            TaskStatus::InProgress => Self::InProgress,
            TaskStatus::Done => Self::Done,
            TaskStatus::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    /// Tasks this task depends on, in ascending ID order.
    pub depends_on: Vec<TaskId>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub herdr_workspace_id: Option<String>,
    pub pull_request_url: Option<String>,
}

impl From<&model::Task> for Task {
    fn from(task: &model::Task) -> Self {
        Self {
            id: task.id.into(),
            workspace_id: task.workspace_id.into(),
            task_type: task.task_type.into(),
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status.into(),
            depends_on: task.depends_on.iter().copied().map(TaskId::from).collect(),
            branch: task.branch.clone(),
            worktree_path: task.worktree_path.as_ref().map(ToString::to_string),
            herdr_workspace_id: task.herdr_workspace_id.clone(),
            pull_request_url: task.pull_request_url.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    pub description: String,
}

impl TryFrom<NewTask> for storage::NewTask {
    type Error = CommandError;

    fn try_from(new: NewTask) -> CommandResult<Self> {
        Ok(Self {
            workspace_id: new.workspace_id.into(),
            task_type: new.task_type.into(),
            title: required("task title", new.title)?,
            description: new.description,
        })
    }
}

/// The fields of a task the user edits directly. Status, dependencies, and the
/// worktree, herdr, and pull request fields have their own commands or workflow steps.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TaskEdit {
    pub id: TaskId,
    pub task_type: TaskType,
    pub title: String,
    pub description: String,
}

/// A task whose status moved between `planned` and `ready` because of a change to
/// its dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StatusChange {
    pub task_id: TaskId,
    pub from: TaskStatus,
    pub to: TaskStatus,
}

impl From<&dag::StatusChange> for StatusChange {
    fn from(change: &dag::StatusChange) -> Self {
        Self {
            task_id: change.task.into(),
            from: change.from.into(),
            to: change.to.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommitRecord {
    pub task_id: TaskId,
    pub sha: String,
    pub message: String,
    pub author: String,
    /// Commit timestamp in seconds since the Unix epoch.
    #[specta(type = Number)]
    pub committed_at: i64,
}

impl From<&model::CommitRecord> for CommitRecord {
    fn from(record: &model::CommitRecord) -> Self {
        Self {
            task_id: record.task_id.into(),
            sha: record.sha.clone(),
            message: record.message.clone(),
            author: record.author.clone(),
            committed_at: record.committed_at,
        }
    }
}

pub(super) fn map_all<'a, T: 'a, U: From<&'a T>>(items: &'a [T]) -> Vec<U> {
    items.iter().map(U::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandErrorKind;

    #[test]
    fn local_location_is_normalized() {
        let location = model::WorkspaceLocation::try_from(WorkspaceLocation::Local {
            path: "C:/dev/toodo/".to_owned(),
        })
        .unwrap();
        assert_eq!(
            WorkspaceLocation::from(&location),
            WorkspaceLocation::Local {
                path: r"C:\dev\toodo".to_owned()
            }
        );
    }

    #[test]
    fn location_paths_must_match_the_machine() {
        let cases = [
            WorkspaceLocation::Local {
                path: "/home/me/toodo".to_owned(),
            },
            WorkspaceLocation::Ssh {
                machine: "devbox".to_owned(),
                path: r"C:\dev\toodo".to_owned(),
            },
            WorkspaceLocation::Ssh {
                machine: " ".to_owned(),
                path: "/home/me/toodo".to_owned(),
            },
        ];
        for location in cases {
            let error = model::WorkspaceLocation::try_from(location).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        }
    }

    #[test]
    fn new_workspace_requires_name_and_base_branch() {
        let new = |name: &str, branch: &str| NewWorkspace {
            name: name.to_owned(),
            location: WorkspaceLocation::Local {
                path: r"C:\dev\toodo".to_owned(),
            },
            default_base_branch: branch.to_owned(),
        };
        let parsed = storage::NewWorkspace::try_from(new(" toodo ", " main ")).unwrap();
        assert_eq!(parsed.name, "toodo");
        assert_eq!(parsed.default_base_branch, "main");
        for (name, branch) in [("  ", "main"), ("toodo", "")] {
            let error = storage::NewWorkspace::try_from(new(name, branch)).unwrap_err();
            assert_eq!(error.kind, CommandErrorKind::InvalidInput);
        }
    }

    #[test]
    fn machine_settings_parse_the_root_for_their_machine() {
        let settings = model::MachineSettings::try_from(MachineSettings {
            machine: Machine::Ssh {
                label: "devbox".to_owned(),
            },
            worktree_root: Some("/srv/worktrees/".to_owned()),
        })
        .unwrap();
        assert_eq!(
            MachineSettings::from(&settings),
            MachineSettings {
                machine: Machine::Ssh {
                    label: "devbox".to_owned()
                },
                worktree_root: Some("/srv/worktrees".to_owned()),
            }
        );
        let error = model::MachineSettings::try_from(MachineSettings {
            machine: Machine::Local,
            worktree_root: Some("/srv/worktrees".to_owned()),
        })
        .unwrap_err();
        assert_eq!(error.kind, CommandErrorKind::InvalidInput);
    }

    #[test]
    fn serialized_shapes_use_the_frontend_names() {
        let location = serde_json::to_value(WorkspaceLocation::Ssh {
            machine: "devbox".to_owned(),
            path: "/srv/toodo".to_owned(),
        })
        .unwrap();
        assert_eq!(
            location,
            serde_json::json!({ "kind": "ssh", "machine": "devbox", "path": "/srv/toodo" })
        );
        assert_eq!(
            serde_json::to_value(TaskStatus::InProgress).unwrap(),
            "in_progress"
        );
        let edit: TaskEdit = serde_json::from_value(serde_json::json!({
            "id": 7, "taskType": "refactor", "title": "Split storage", "description": ""
        }))
        .unwrap();
        assert_eq!(edit.id, TaskId(7));
        assert_eq!(edit.task_type, TaskType::Refactor);
    }
}
