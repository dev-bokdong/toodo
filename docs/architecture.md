# Architecture

This document describes the domain model and the main components of Toodo. It is a
starting point; refine it as implementation decisions are made.

## Domain Model

### Workspace

A root git repository that tasks are planned against.

| Field | Description |
| --- | --- |
| `id` | Stable identifier. |
| `name` | Display name. |
| `kind` | `local` or `ssh`. |
| `path` | Absolute path to the repository root: a Windows path for `local`, a POSIX path for `ssh`. Keep the two as distinct types. |
| `herdrMachine` | For `ssh` workspaces: the saved herdr machine label; also used as the `ssh.exe` host. |
| `defaultBaseBranch` | Branch that task branches start from and PRs target (e.g. `main`). |

### Machine Settings

Per-machine configuration. There is one entry for the local machine and one for each
saved herdr SSH machine that hosts a workspace.

| Field | Description |
| --- | --- |
| `machine` | `local` or the saved herdr machine label. |
| `worktreeRoot` | Absolute directory under which task worktrees are created on that machine (Windows path for `local`, POSIX path for SSH machines). Required before a task on that machine can be started. |

### Task Types

A fixed set; users cannot define their own types for now.

| Type | Use for |
| --- | --- |
| `feat` | A new feature. |
| `fix` | A bug fix. |
| `modify` | A change to existing behavior that is neither a new feature nor a bug fix. |
| `refactor` | Restructuring code without changing behavior. |
| `docs` | Documentation-only changes. |
| `test` | Adding or changing tests only. |
| `chore` | Build, tooling, dependency, or maintenance work. |

### Worktree Layout and Branch Naming

Each task worktree is created at:

```
<worktreeRoot>/<workspace-name>/<task-type>-<task-name>-<task-id>
```

and checks out the task branch:

```
<task-type>/<task-name>-<task-id>
```

- `worktreeRoot` comes from the settings of the machine that hosts the workspace.
- `<workspace-name>` is the source workspace's name.
- `<task-type>` is the task's [type](#task-types).
- `<task-name>` is the task title; `<task-id>` keeps the path and branch unique even when
  titles repeat.
- Workspace and task names are converted to slugs that are safe both as directory names
  (no path separators, no characters invalid on Windows, no trailing dots or spaces) and
  as git ref components.
- Path and branch are passed to herdr explicitly:
  `herdr worktree create --path <path> --branch <branch>`.

### Task

A unit of work inside a workspace.

| Field | Description |
| --- | --- |
| `id` | Stable identifier. |
| `workspaceId` | Owning workspace. |
| `type` | Kind of change, one of the [task types](#task-types). Used in the worktree path and branch name. |
| `title`, `description` | What should be implemented (English). |
| `status` | See [workflow.md](workflow.md#task-states). |
| `dependsOn` | IDs of tasks that must be completed first. |
| `branch` | Branch created for the task's worktree. |
| `worktreePath` | Directory of the task's worktree. |
| `herdrWorkspaceId` | herdr workspace bound to the worktree, if any. |
| `pullRequestUrl` | Set when the PR is opened. |

### Task Dependencies (DAG)

- `dependsOn` edges must stay within one workspace and must never form a cycle.
- A task is **ready** when every task it depends on is `done`.
- Dependents of a task can be planned at any time but only started once ready.
- A topological order of the DAG is the suggested execution order.

### CommitRecord

A commit produced while implementing a task.

| Field | Description |
| --- | --- |
| `taskId` | Task the commit belongs to. |
| `sha` | Full commit hash. |
| `message` | Commit subject (and body if available). |
| `author` | Commit author. |
| `committedAt` | Commit timestamp. |

Commits are discovered from the task branch, typically as
`git log <baseBranch>..<taskBranch>` run in the worktree.

## Runtime Context

Toodo is a Tauri 2 Windows desktop app that runs next to an already installed and running
herdr. herdr owns terminals, agents, worktree checkouts, and SSH machine connections;
Toodo owns planning, task state, commit records, and pull requests.

## Components

```
+-----------------+      +-------------------+      +------------------+
| Toodo UI        | ---> | Toodo Core (Rust) | ---> | Storage (SQLite) |
| React + React   |      | (tasks, DAG rules,|      | (workspaces,     |
| Flow (WebView2) |      |  lifecycle)       |      |  tasks, commits) |
+-----------------+      +-------------------+      +------------------+
                                  |
          +-----------------------+------------------------+
          v                       v                        v
+-------------------+   +-------------------+   +----------------------+
| Workspace adapter |   | herdr adapter     |   | Git hosting adapter  |
| (git.exe locally, |   | (CLI commands +   |   | (gh CLI: push and    |
|  ssh.exe + git    |   |  named-pipe event |   |  open pull requests) |
|  for remote)      |   |  subscription)    |   |                      |
+-------------------+   +-------------------+   +----------------------+
```

- **Toodo Core**: owns the domain rules: DAG validation, readiness, and the task state
  machine. It has no direct I/O; it calls adapters.
- **Workspace adapter**: runs `git.exe` for local workspaces and `ssh.exe <machine> git ...`
  for remote ones. Uses the user's `~/.ssh/config` and Windows ssh-agent, the same
  configuration herdr uses.
- **herdr adapter**: wraps herdr, which is a required, preinstalled dependency.
  - Startup health check: `herdr` on `PATH` and the server answers `ping`.
  - Commands use the herdr CLI (recommended by herdr for portable Windows behavior).
  - Events use the socket API: newline-delimited JSON over a Windows named pipe,
    `events.subscribe` for `worktree.created`, `worktree.removed`, and
    `pane.agent_status_changed`. On `events_lost`, resubscribe and reconcile with
    `session.snapshot`.
  - Toodo does not embed a terminal; it focuses the herdr workspace instead.
  - Relevant commands:
  - `herdr worktree create|open|list|remove`: manage worktree-backed herdr workspaces.
  - `herdr workspace list|get|focus|close`: inspect and control herdr workspaces.
  - `herdr agent list|get|prompt|wait|read`: drive and observe agents in panes.
  - `herdr machine list|status|add`: saved SSH machines for remote workspaces.
  - `herdr api schema` / `herdr api snapshot`: API schema and live runtime state.
- **Git hosting adapter**: pushes the task branch and opens pull requests with the `gh`
  CLI, reusing its authentication.
- **Storage**: SQLite database in the app data directory (`%APPDATA%`) for workspaces,
  tasks, dependencies, and commit records.

## Decisions

- Target: Windows desktop app built with Tauri 2.
- herdr is a prerequisite; Toodo only checks that it is running and never installs it.
- Backend in Rust (tokio); frontend in React + TypeScript with React Flow for the DAG.
- External tools are invoked as processes (`herdr`, `git.exe`, `ssh.exe`, `gh`) rather than
  reimplemented through libraries.
- Worktree location is configured per machine (`worktreeRoot`); worktree paths and branch
  names are derived from task type, name, and ID; see
  [Worktree Layout and Branch Naming](#worktree-layout-and-branch-naming).
- herdr CLI output is parsed as JSON. herdr CLI commands print the same
  `{"id": ..., "result": ...}` envelope as the socket API (verified for `workspace list`,
  `worktree list`, and `agent list` on herdr 0.9.3). Any command found not to return JSON
  is sent over the named pipe instead.
- Commits are recorded automatically: on task completion Toodo runs
  `git log <baseBranch>..<taskBranch>` in the worktree and stores the result. While a task
  is in progress, the user can refresh commit records manually.
- Storage uses SQLite through `rusqlite` (single-user local app; no async pool needed).
- Pull requests target GitHub only, through the `gh` CLI.
- Repositories inside WSL (`\\wsl$\...`) are not supported as local workspaces. Register
  them as SSH workspaces if needed.
- Task types are a fixed set (see [Task Types](#task-types)).

## Deferred

Out of the first scope; revisit when needed.

- User-defined task types.
- Git hosting providers beyond GitHub (for example GitLab), added as another Git hosting
  adapter.
- Code signing for the installer; decide before public distribution (unsigned builds
  trigger SmartScreen warnings).
- A herdr plugin for in-herdr actions such as "complete task"; herdr plugins are in
  preview on Windows.
- Native WSL repository support.
