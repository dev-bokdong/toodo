//! Worktree directory and branch names for a task:
//! `<worktreeRoot>/<workspace-slug>/<task-type>-<task-slug>-<task-id>` checked out on
//! `<task-type>/<task-slug>-<task-id>`.

use std::fmt;

use super::model::{HostPath, Machine, MachineSettings, Task, Workspace, WorkspaceLocation};
use super::slug::slugify;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCheckout {
    pub path: HostPath,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamingError {
    TaskNotInWorkspace,
    WorktreeRootNotSet(Machine),
    MachineMismatch {
        workspace: Machine,
        settings: Machine,
    },
}

impl fmt::Display for NamingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskNotInWorkspace => f.write_str("task does not belong to the workspace"),
            Self::WorktreeRootNotSet(machine) => {
                write!(f, "worktree root is not set for machine {machine}")
            }
            Self::MachineMismatch {
                workspace,
                settings,
            } => write!(
                f,
                "workspace is on machine {workspace} but the settings are for machine {settings}"
            ),
        }
    }
}

impl std::error::Error for NamingError {}

pub fn task_checkout(
    workspace: &Workspace,
    settings: &MachineSettings,
    task: &Task,
) -> Result<TaskCheckout, NamingError> {
    if task.workspace_id != workspace.id {
        return Err(NamingError::TaskNotInWorkspace);
    }
    let workspace_dir = slugify(&workspace.name, "workspace");
    let task_name = format!("{}-{}", slugify(&task.title, "task"), task.id);
    let task_dir = format!("{}-{task_name}", task.task_type);
    let branch = format!("{}/{task_name}", task.task_type);

    let path = match (&workspace.location, settings) {
        (WorkspaceLocation::Local { .. }, MachineSettings::Local { worktree_root }) => {
            let root = worktree_root
                .as_ref()
                .ok_or(NamingError::WorktreeRootNotSet(Machine::Local))?;
            HostPath::Windows(root.join(&workspace_dir).join(&task_dir))
        }
        (
            WorkspaceLocation::Ssh { machine, .. },
            MachineSettings::Ssh {
                machine: settings_machine,
                worktree_root,
            },
        ) if machine == settings_machine => {
            let root = worktree_root
                .as_ref()
                .ok_or_else(|| NamingError::WorktreeRootNotSet(Machine::Ssh(machine.clone())))?;
            HostPath::Posix(root.join(&workspace_dir).join(&task_dir))
        }
        _ => {
            return Err(NamingError::MachineMismatch {
                workspace: workspace.location.machine(),
                settings: settings.machine(),
            });
        }
    };
    Ok(TaskCheckout { path, branch })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::domain::model::{MachineLabel, TaskId, TaskType, WorkspaceId};
    use crate::domain::path::{PosixPath, WindowsPath};
    use crate::domain::status::TaskStatus;

    fn task(task_type: TaskType, title: &str) -> Task {
        Task {
            id: TaskId(42),
            workspace_id: WorkspaceId(1),
            task_type,
            title: title.to_owned(),
            description: String::new(),
            status: TaskStatus::Planned,
            depends_on: BTreeSet::new(),
            branch: None,
            worktree_path: None,
            herdr_workspace_id: None,
            pull_request_url: None,
            pull_request_merged_at: None,
            pull_request: None,
            resolution: None,
            completion_note: None,
            estimate: None,
            due_date: None,
            issue: None,
            timeline: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn local_workspace(name: &str) -> Workspace {
        Workspace {
            id: WorkspaceId(1),
            name: name.to_owned(),
            location: WorkspaceLocation::Local {
                path: WindowsPath::parse(r"C:\src\toodo").unwrap(),
            },
            default_base_branch: "main".to_owned(),
        }
    }

    fn ssh_workspace(machine: &str) -> Workspace {
        Workspace {
            id: WorkspaceId(1),
            name: "Toodo".to_owned(),
            location: WorkspaceLocation::Ssh {
                machine: MachineLabel::parse(machine).unwrap(),
                path: PosixPath::parse("/home/me/toodo").unwrap(),
            },
            default_base_branch: "main".to_owned(),
        }
    }

    fn local_settings(root: Option<&str>) -> MachineSettings {
        MachineSettings::Local {
            worktree_root: root.map(|r| WindowsPath::parse(r).unwrap()),
        }
    }

    fn ssh_settings(machine: &str, root: Option<&str>) -> MachineSettings {
        MachineSettings::Ssh {
            machine: MachineLabel::parse(machine).unwrap(),
            worktree_root: root.map(|r| PosixPath::parse(r).unwrap()),
        }
    }

    #[test]
    fn local_task_checkout_uses_windows_separators() {
        let checkout = task_checkout(
            &local_workspace("My Repo"),
            &local_settings(Some(r"D:\worktrees")),
            &task(TaskType::Feat, "Add login page"),
        )
        .unwrap();
        assert_eq!(
            checkout.path.to_string(),
            r"D:\worktrees\my-repo\feat-add-login-page-42"
        );
        assert_eq!(checkout.branch, "feat/add-login-page-42");
    }

    #[test]
    fn ssh_task_checkout_uses_posix_separators() {
        let checkout = task_checkout(
            &ssh_workspace("devbox"),
            &ssh_settings("devbox", Some("/srv/wt/")),
            &task(TaskType::Fix, "Crash on empty DAG"),
        )
        .unwrap();
        assert_eq!(
            checkout.path.to_string(),
            "/srv/wt/toodo/fix-crash-on-empty-dag-42"
        );
        assert_eq!(checkout.branch, "fix/crash-on-empty-dag-42");
    }

    #[test]
    fn unusable_names_fall_back_to_placeholders() {
        let checkout = task_checkout(
            &local_workspace("CON"),
            &local_settings(Some(r"C:\wt")),
            &task(TaskType::Docs, "문서 정리"),
        )
        .unwrap();
        assert_eq!(
            checkout.path.to_string(),
            r"C:\wt\con-workspace\docs-task-42"
        );
        assert_eq!(checkout.branch, "docs/task-42");
    }

    #[test]
    fn missing_worktree_root_is_reported_per_machine() {
        let feat = task(TaskType::Feat, "x");
        assert_eq!(
            task_checkout(&local_workspace("a"), &local_settings(None), &feat),
            Err(NamingError::WorktreeRootNotSet(Machine::Local))
        );
        assert_eq!(
            task_checkout(
                &ssh_workspace("devbox"),
                &ssh_settings("devbox", None),
                &feat
            ),
            Err(NamingError::WorktreeRootNotSet(Machine::Ssh(
                MachineLabel::parse("devbox").unwrap()
            )))
        );
    }

    #[test]
    fn settings_for_another_machine_are_rejected() {
        let feat = task(TaskType::Feat, "x");
        assert!(matches!(
            task_checkout(
                &ssh_workspace("devbox"),
                &ssh_settings("other", Some("/wt")),
                &feat
            ),
            Err(NamingError::MachineMismatch { .. })
        ));
        assert!(matches!(
            task_checkout(
                &local_workspace("a"),
                &ssh_settings("devbox", Some("/wt")),
                &feat
            ),
            Err(NamingError::MachineMismatch { .. })
        ));
    }

    #[test]
    fn task_from_another_workspace_is_rejected() {
        let mut foreign = task(TaskType::Feat, "x");
        foreign.workspace_id = WorkspaceId(2);
        assert_eq!(
            task_checkout(
                &local_workspace("a"),
                &local_settings(Some(r"C:\wt")),
                &foreign
            ),
            Err(NamingError::TaskNotInWorkspace)
        );
    }
}
