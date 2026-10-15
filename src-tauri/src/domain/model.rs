//! Core domain types: workspaces, machines, tasks, and commit records.

use std::collections::BTreeSet;
use std::fmt;

use super::path::{PosixPath, WindowsPath};
use super::status::TaskStatus;
use super::timeline::TaskEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub i64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Label of a saved herdr SSH machine; also used as the `ssh.exe` host.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MachineLabel(String);

impl MachineLabel {
    pub fn parse(raw: &str) -> Result<Self, EmptyMachineLabel> {
        let label = raw.trim();
        if label.is_empty() {
            return Err(EmptyMachineLabel);
        }
        Ok(Self(label.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MachineLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyMachineLabel;

impl fmt::Display for EmptyMachineLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("machine label must not be empty")
    }
}

impl std::error::Error for EmptyMachineLabel {}

/// The machine that hosts a workspace and its task worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Machine {
    Local,
    Ssh(MachineLabel),
}

impl fmt::Display for Machine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Ssh(label) => label.fmt(f),
        }
    }
}

/// Where a workspace repository lives. Local workspaces use Windows paths, SSH
/// workspaces use POSIX paths on the remote machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceLocation {
    Local {
        path: WindowsPath,
    },
    Ssh {
        machine: MachineLabel,
        path: PosixPath,
    },
}

impl WorkspaceLocation {
    pub fn machine(&self) -> Machine {
        match self {
            Self::Local { .. } => Machine::Local,
            Self::Ssh { machine, .. } => Machine::Ssh(machine.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub location: WorkspaceLocation,
    pub default_base_branch: String,
}

/// Per-machine configuration. `worktree_root` must be set before a task on that
/// machine can be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineSettings {
    Local {
        worktree_root: Option<WindowsPath>,
    },
    Ssh {
        machine: MachineLabel,
        worktree_root: Option<PosixPath>,
    },
}

impl MachineSettings {
    pub fn machine(&self) -> Machine {
        match self {
            Self::Local { .. } => Machine::Local,
            Self::Ssh { machine, .. } => Machine::Ssh(machine.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostPath {
    Windows(WindowsPath),
    Posix(PosixPath),
}

impl fmt::Display for HostPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Windows(path) => path.fmt(f),
            Self::Posix(path) => path.fmt(f),
        }
    }
}

/// Fixed set of task types; used in worktree directory and branch names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskType {
    Feat,
    Fix,
    Modify,
    Refactor,
    Docs,
    Test,
    Chore,
    /// Research and development: work whose conclusion may be not to adopt it.
    Rnd,
}

impl TaskType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feat => "feat",
            Self::Fix => "fix",
            Self::Modify => "modify",
            Self::Refactor => "refactor",
            Self::Docs => "docs",
            Self::Test => "test",
            Self::Chore => "chore",
            Self::Rnd => "rnd",
        }
    }
}

impl fmt::Display for TaskType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a `done` task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskResolution {
    /// Its pull request was merged into the base branch.
    Merged,
    /// The work was concluded without merging, e.g. research whose result was not
    /// adopted. Its completion note says why; dependents stay blocked.
    NotAdopted,
}

impl TaskResolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::NotAdopted => "not_adopted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: WorkspaceId,
    pub task_type: TaskType,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    /// How the task ended; `Some` exactly when it is `done`.
    pub resolution: Option<TaskResolution>,
    pub depends_on: BTreeSet<TaskId>,
    pub branch: Option<String>,
    pub worktree_path: Option<HostPath>,
    pub herdr_workspace_id: Option<String>,
    pub pull_request_url: Option<String>,
    /// The GitHub issue of the workspace repository this task works on; its pull request
    /// closes it on merge.
    pub issue: Option<LinkedIssue>,
    /// Merge time of the pull request reported by GitHub, in seconds since the Unix
    /// epoch.
    pub pull_request_merged_at: Option<i64>,
    /// What the pull request looked like when its merge completed the task.
    pub pull_request: Option<PullRequestSnapshot>,
    /// What the user wrote about the outcome of a `done` task, for later reviews.
    pub completion_note: Option<String>,
    /// Status history, oldest event first.
    pub timeline: Vec<TaskEvent>,
}

/// A GitHub issue linked to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedIssue {
    pub number: i64,
    pub url: String,
}

/// A merged pull request as GitHub reported it when it completed its task, kept so
/// reviews do not depend on GitHub later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestSnapshot {
    pub number: i64,
    pub title: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    /// The commit the merge put on the base branch; with a squash merge the branch
    /// commits never reach it.
    pub merge_commit_sha: Option<String>,
    /// Submitted reviews by anyone but the author of the pull request.
    pub review_count: i64,
    /// Distinct reviewers other than the author of the pull request.
    pub reviewer_count: i64,
    /// Submission time of the first of those reviews, in seconds since the Unix epoch.
    pub first_reviewed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub task_id: TaskId,
    pub sha: String,
    pub message: String,
    pub author: String,
    /// Commit timestamp in seconds since the Unix epoch.
    pub committed_at: i64,
}
