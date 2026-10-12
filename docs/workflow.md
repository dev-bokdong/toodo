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
machine (its SSH target plus POSIX path), and one of two sources:

- **Clone**: Toodo runs `git clone <url> <path>`. The path must not exist or must be an
  empty directory.
- **Open**: the path already holds a cloned repository. Toodo checks it with
  `git rev-parse --show-toplevel` and records the repository root.

A remote machine is added once from the sidebar: the user picks a host from `~/.ssh/config`
(or adds a new `Host` entry) and Toodo runs `herdr machine add <host>` in a console window
where the user answers herdr's setup prompts.

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
3. Calls `herdr worktree create --cwd <repository> --path <dir> --branch <branch>
   --base <baseBranch> --no-focus` (prefixed with `--machine <label>` for SSH
   workspaces) to create the git worktree and open it as a herdr workspace. If herdr
   fails, the task stays `ready` and nothing is stored.
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
(`git log --reverse <baseBranch>..<taskBranch>` in the worktree, over `ssh.exe` for remote
workspaces) and stores a `CommitRecord` (SHA, message, author, timestamp) for each one,
oldest first, replacing the previous records. This runs automatically when the task is
completed; while the task is in progress, the user can trigger it manually ("Refresh
commits") to refresh the list.

### 6. Complete the task and open a PR

When the user marks an `in_progress` task complete, Toodo:

1. Reads the commits of the task branch. A branch without commits ahead of the base
   branch is rejected, because there is nothing to open a pull request for.
2. Reads the `origin` remote URL and derives the GitHub repository (`host/owner/name`)
   from it. A remote that is not a hosted repository URL is rejected.
3. Pushes the task branch to `origin` (`git push --set-upstream origin <branch>`, over
   `ssh.exe` for remote workspaces).
4. Reuses the open pull request of the task branch if there is one
   (`gh pr list --repo <repo> --head <branch> --state open`); otherwise opens one with
   `gh pr create --repo <repo> --head <branch> --base <baseBranch>`, titled with the task
   title and described with the task description. `gh` always runs on this machine, so
   SSH machines do not need it.
5. In one transaction, stores the commit records and the pull request URL and moves the
   task to `done`, which may make dependent tasks `ready`.

If any step fails, the task stays `in_progress` with its previous commit records and the
error is shown; completing it again is safe, because pushing is repeatable and an open
pull request is reused instead of created twice.

Removing the worktree (`herdr worktree remove`) is a separate cleanup step, usually after
the pull request is merged.
