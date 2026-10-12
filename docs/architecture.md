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

### Task

A unit of work inside a workspace.

| Field | Description |
| --- | --- |
| `id` | Stable identifier. |
| `workspaceId` | Owning workspace. |
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

## Open Decisions

- Whether repositories inside WSL (`\\wsl$\...`) are supported (suggested: not initially;
  treat them as SSH workspaces if needed).
- Whether the herdr CLI offers machine-readable output for every command needed; if not,
  send those requests over the named pipe directly.
- Whether commit recording is automatic (on task completion / on events) or manual.
- Worktree directory layout and branch naming scheme.
- Git hosting providers beyond GitHub.
- Code signing for the installer (unsigned builds trigger SmartScreen warnings).
- An optional herdr plugin (in-herdr actions such as "complete task"); plugins are in
  preview on Windows, so this is out of the first scope.
