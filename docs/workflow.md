# Task Workflow

This document describes how a task moves from planning to a pull request.

## Task States

```
planned --> ready --> in_progress --> done
   ^          |            |
   |          +------------+--> cancelled
   +-- (a dependency is added that is not done yet)
```

A `planned` task can also be cancelled. `done` and `cancelled` are final states.

| State | Meaning |
| --- | --- |
| `planned` | Task exists; one or more dependencies are not `done` yet. |
| `ready` | All dependencies are `done`; the task can be started. |
| `in_progress` | A worktree exists and implementation is happening in herdr. |
| `done` | Task completed; commits recorded and a pull request opened. |
| `cancelled` | Task abandoned; its worktree may be removed. |

## Lifecycle

### 1. Register a workspace

The user picks a local Windows directory, or a remote directory on a saved herdr SSH
machine (machine label plus POSIX path), and one of two sources:

- **Clone**: Toodo runs `git clone <url> <path>`. The path must not exist or must be an
  empty directory.
- **Open**: the path already holds a cloned repository. Toodo checks it with
  `git rev-parse --show-toplevel` and records the repository root.

Git runs locally as `git.exe` and remotely as `ssh.exe <machine> git ...`. The default
base branch is the one the user enters, or else the branch `origin/HEAD` points to, or
else the checked-out branch. Deleting a workspace never deletes its repository.

### 2. Plan tasks

The user creates tasks and declares dependencies between them. Toodo rejects any
dependency that would create a cycle and computes which tasks are `ready`.

### 3. Prepare the worktree

When the user starts a `ready` task, Toodo:

1. Resolves the worktree directory as
   `<worktreeRoot>/<workspace-name>/<task-type>-<task-name>-<task-id>`, where
   `worktreeRoot` is set per machine (see
   [architecture.md](architecture.md#worktree-layout-and-branch-naming)). If the machine
   has no `worktreeRoot` yet, the user is asked to set it first.
2. Derives the task branch as `<task-type>/<task-name>-<task-id>`.
3. Calls `herdr worktree create --path <dir> --branch <branch> --base <baseBranch>` to
   create the git worktree and open it as a herdr workspace.
4. Stores `branch`, `worktreePath`, and `herdrWorkspaceId` on the task and moves it to
   `in_progress`.

### 4. Implement in herdr

Implementation happens directly in herdr: the user and/or AI agents work in panes bound
to the task's worktree. Toodo does not embed a terminal; it focuses the herdr workspace
(`herdr workspace focus`). Toodo may optionally send the task description as a prompt to
an agent (`herdr agent prompt`) and shows agent state (working, blocked, done) from
`pane.agent_status_changed` events.

### 5. Record commits

Toodo reads the commits on the task branch that are not on the base branch
(`git log <baseBranch>..<taskBranch>`) and stores a `CommitRecord` (SHA, message, author,
timestamp) for each one. This runs automatically when the task is completed; while the
task is in progress, the user can trigger it manually to refresh the list.

### 6. Complete the task and open a PR

When the user marks the task complete, Toodo:

1. Refreshes the commit records.
2. Pushes the task branch to the git remote (over `ssh.exe` for remote workspaces).
3. Opens a pull request with `gh` from the task branch into the base branch and stores
   its URL.
4. Moves the task to `done`, which may make dependent tasks `ready`.

Removing the worktree (`herdr worktree remove`) is a separate cleanup step, usually after
the pull request is merged.
