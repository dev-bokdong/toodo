# Architecture

This document describes the domain model and the main components of Toodo. It is a
starting point; refine it as implementation decisions are made.

## Purpose

Besides driving the work itself, Toodo is the record used to compile a year-end
performance report. For every task it must be able to answer when it was planned, when
work on it started, when its pull request was opened, and when it was completed or
cancelled, and show how tasks depended on each other over time. Timeline data is
therefore part of the domain, not a UI detail: it is recorded at the moment each change
happens and never derived later from git or GitHub alone.

## Domain Model

### Workspace

A root git repository that tasks are planned against. A workspace is created either by
cloning a repository into a path or by opening a repository already cloned at a path; see
[workflow.md](workflow.md#1-register-a-workspace).

| Field | Description |
| --- | --- |
| `id` | Stable identifier. |
| `name` | Display name. |
| `kind` | `local` or `ssh`. |
| `path` | Absolute path to the repository root: a Windows path for `local`, a POSIX path for `ssh`. Keep the two as distinct types. |
| `herdrMachine` | For `ssh` workspaces: the SSH target of the saved herdr machine (usually a `~/.ssh/config` host alias); also used as the `ssh.exe` host. |
| `defaultBaseBranch` | Branch that task branches start from and PRs target (e.g. `main`). |

### Machine Settings

Per-machine configuration. There is one entry for the local machine and one for each
saved herdr SSH machine that hosts a workspace.

| Field | Description |
| --- | --- |
| `machine` | `local` or the SSH target of the saved herdr machine. |
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
  as git ref components: lowercase ASCII letters and digits joined by single hyphens,
  at most 48 characters. Other characters, including non-ASCII letters, separate words.
  An empty result falls back to `workspace` or `task`, and Windows device names such as
  `con` get that word as a suffix (`con-workspace`).
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
| `pullRequestUrl` | Set when the PR is opened, as the task moves to `in_review`. |
| `pullRequestMergedAt` | Merge time reported by GitHub (`mergedAt`), stored when a merged pull request is recorded. |
| `timeline` | Status history of the task; see [Task Timeline](#task-timeline). |

### Task Timeline

Every task keeps an append-only status history. Each entry is a `TaskEvent`:

| Field | Description |
| --- | --- |
| `taskId` | Task the event belongs to. |
| `status` | Status the task moved to. |
| `at` | Time of the change, in seconds since the Unix epoch (UTC), from the clock of the machine running Toodo. Shown in the local time zone. |

- Creating a task records its initial status (`planned` or `ready`). Every later status
  change, including the automatic `planned`/`ready` recomputation after dependency
  changes, records one event in the same transaction as the change itself.
- Events are never edited or deleted on their own; they go away only when their task is
  deleted. Cancelling a task keeps its history, so cancel rather than delete tasks that
  should appear in a report.
- The milestones used by the timeline view and reports are derived from the history:

  | Milestone | Derived from |
  | --- | --- |
  | `plannedAt` | The first event (task creation). |
  | `readyAt` | The latest move to `ready` before `startedAt` (when the last dependency was done). |
  | `startedAt` | The first move to `in_progress`. |
  | `reviewAt` | The first move to `in_review` (pull request opened). |
  | `completedAt` | The move to `done`. |
  | `cancelledAt` | The move to `cancelled`. |

- Tasks created before status history existed have no events; their milestones are
  unknown and are not guessed from git or GitHub.

### Task Dependencies (DAG)

- There is one DAG across all workspaces: a task may depend on a task of another
  workspace, for example an application change that needs a low-level library change
  first. Edges must never form a cycle, including cycles that pass through several
  workspaces.
- A task is **ready** when every task it depends on is `done`, in whichever workspace it
  lives; finishing a task can release dependents in other workspaces.
- A workspace cannot be deleted while a `planned` or `ready` task of another workspace
  depends on one of its tasks, since that task would silently become ready. Dependencies
  of tasks that already started are dropped with the workspace.
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

## Timeline View

The dependency graph of a workspace is drawn on a time axis instead of as a free-form
layered DAG, so the same picture shows both what depended on what and when it happened.
Prerequisites from other workspaces are drawn too, labeled with their workspace, and an
"All workspaces" view draws the tasks of every workspace together.

- It reads like a Gantt chart: a time axis header (coarse dates above fine dates or
  hours) stays at the top while the rows scroll, and a cursor marks the current time,
  refreshed every 30 seconds.
- Each task is a bar whose ends follow its status:
  - Not started (`planned`, `ready`): a fixed-width bar attached to the current-time
    cursor, moving with it.
  - Started (`in_progress`, `in_review`): fixed at `startedAt`, its end attached to the
    current-time cursor.
  - Closed (`done`, `cancelled`): fixed from `startedAt` to `completedAt` (or
    `cancelledAt`).
  A started bar is split into phases: in progress (`startedAt` to `reviewAt`) and in
  review (`reviewAt` to the end).
- The wait before a bar is drawn lighter than the bar, so bar length still reads as
  work: a marker at `plannedAt` and a dashed line from it to the bar, with a tick at
  `readyAt` when the task became ready later than it was planned (its prerequisites
  were done then). The axis starts early enough to show the earliest `plannedAt`.
- Every task has its own row. Rows are in dependency order: a task comes after all the
  tasks it depends on, otherwise in order of start time. A task that has not started is
  also placed to the right of its prerequisites' bars, so a chain of waiting tasks reads
  left to right from the current time.
- Dependency edges run from the prerequisite's bar to the dependent's bar.
- The visible range can be zoomed (hour, day, week, month) and narrowed to a period such
  as a calendar year, the unit of a performance report.
- Started or closed tasks without the milestones to place them (tasks created before
  status history existed) are listed beside the timeline instead of on it.
- Dependencies can still be added and removed in this view, with the same rules as in
  the task list.

## Review

A report of the work recorded in a period, across all workspaces, for status updates and
for the year-end report. It is built only from recorded facts (status history, commit
records, pull request URLs); nothing is generated or guessed by an agent.

- The user picks a week (from any first day, by default the Monday of the current week),
  a calendar month, quarter, or year, or a custom range of days. The period runs from
  local midnight of its first day to local midnight after its last day, as the half-open
  period `[start, end)` passed to the `review_period` command.
- A task is in the review when it reached a milestone or got a commit in the period.
  Its outcome is the most advanced milestone reached in the period, in this order:

  | Outcome | Reached in the period |
  | --- | --- |
  | Completed | `completedAt` |
  | Cancelled | `cancelledAt` |
  | Opened for review | `reviewAt` |
  | Started | `startedAt` |
  | Worked on | No milestone, but at least one commit (`committedAt`) |
  | Planned | `plannedAt` |

- Each task is listed once, under its outcome, with its workspace, the time of the
  outcome, its pull request, and the commits it got in the period.
- Tasks without status history appear only through their commits.
- A summary leads the review: the count of tasks per outcome, the commits made in the
  period, completed tasks by task type and by workspace, and the median durations of the
  tasks completed in the period:

  | Duration | From | To |
  | --- | --- | --- |
  | Lead time | `plannedAt` | `completedAt` |
  | Cycle time | `startedAt` | `completedAt` |
  | Review time | `reviewAt` | `pullRequestMergedAt`, else `completedAt` |

  A task without one of the milestones is left out of that median. A period over several
  calendar months also counts the tasks completed in each month.
- The review can be copied as Markdown, or saved as a Markdown or CSV file (one row per
  task with its milestones as local times; UTF-8 with a byte order mark). The frontend
  builds the file, since it knows the local time zone, and the `save_report` command
  writes it to the path picked in the save dialog; it only writes `.md` and `.csv` files.

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
  - Startup health check: `herdr` on `PATH` and `herdr status --json` reports
    `server.running`. herdr 0.9.3 has no `ping` command.
  - Commands for SSH workspaces run against the remote herdr server with
    `herdr --machine <profile-id> ...`. `--machine` accepts only a saved profile ID or
    label, so the profile is looked up in `herdr machine list --json` by its SSH target
    on every call, preferring an enabled profile on the `default` session.
  - The sidebar lists Local and the machines of `herdr machine list --json`. Adding a
    machine runs `herdr machine add <host>` in a new console window and waits for it to
    close; herdr asks there before installing or replacing the remote herdr. Toodo never
    installs herdr itself.
  - Failures exit non-zero and print `{"error": {"code", "message"}}` on stderr.
  - Commands use the herdr CLI (recommended by herdr for portable Windows behavior).
  - Events use the socket API (`src-tauri/src/adapters/herdr_events.rs`,
    `src-tauri/src/commands/events.rs`): newline-delimited JSON over the server socket
    that `herdr status --json` reports as `server.socket`. On Windows herdr serves it as
    the named pipe `\\.\pipe\<socket path>`; elsewhere it is a Unix domain socket.
    - The herdr server of an SSH machine is reached with `ssh -o BatchMode=yes -W
      <socket path> -- <host>`, which forwards ssh's standard input and output to the
      remote socket. The socket path comes from running the remote herdr over SSH
      (`herdr --session <saved session> status --json`, looked up on `PATH`, then in
      `~/.local/bin`, Homebrew, `/usr/local/bin`, and the Nix profile), because
      `herdr --machine <id> status server --json` reports a `machine:<id>/<session>`
      placeholder instead (verified on herdr 0.9.3).
    - One background thread keeps one `events.subscribe` connection to the local herdr
      server, and one more to the herdr server of each machine that hosts an SSH
      workspace (checked every 5 s; a machine's thread ends when its last workspace is
      deleted). Each subscribes to `worktree.created`, `worktree.opened`, `worktree.removed`,
      `workspace.closed`, `pane.created`, `pane.closed`, `pane.agent_detected`, and
      `pane.agent_status_changed` of every pane. Agent status subscriptions name one
      pane each and a connection takes one subscription, so a new pane means a new
      connection.
    - Lifecycle events arrive as `{"event": "worktree_removed", "data": {...}}`;
      pane-scoped ones keep the dotted name (`pane.agent_status_changed`). Neither
      `workspace.updated` nor `pane.updated` reports agent status changes (verified on
      herdr 0.9.3).
    - herdr IDs are scoped to one server, so a thread only touches the tasks of its own
      machine. `worktree.removed` forgets the worktree and herdr workspace of the task
      bound to that herdr workspace; `worktree.created`/`worktree.opened` stores the new
      herdr workspace of the task with that worktree path (compared case-insensitively
      for Windows paths). The frontend gets a
      `TasksChanged` event and refetches the workspace's tasks.
    - Agent status is read again with `herdr [--machine <id>] pane list` on every pane
      or agent event
      instead of being patched from the event, as herdr recommends. The status of a task
      is the most urgent one among the panes of its herdr workspace (blocked, working,
      done, idle) and reaches the frontend through `list_agent_statuses` and the
      `AgentStatusesChanged` event.
    - When the subscription ends (herdr stopped, `events_lost`, `pane_not_found`), agent
      statuses are cleared and the thread subscribes again after 1 s, doubling up to
      30 s while the server stays unreachable.
  - Toodo does not embed a terminal; it focuses the herdr workspace instead.
  - Relevant commands:
  - `herdr worktree create|open|list|remove`: manage worktree-backed herdr workspaces.
  - `herdr workspace list|get|focus|close`: inspect and control herdr workspaces.
  - `herdr agent list|get|prompt|wait|read`: drive and observe agents in panes.
  - `herdr machine list|status|add`: saved SSH machines for remote workspaces.
  - `herdr api schema` / `herdr api snapshot`: API schema and live runtime state.
- **Git hosting adapter**: opens pull requests with the `gh` CLI, reusing its
  authentication (`src-tauri/src/adapters/github.rs`). `gh` runs on this machine and
  names the repository with `--repo <host>/<owner>/<name>`, derived from the `origin`
  remote URL, so it needs no local checkout. The task branch is pushed by the workspace
  adapter, on the machine that holds the worktree.
- **Storage**: SQLite database `toodo.sqlite3` in the app data directory (`%APPDATA%`)
  for workspaces, machine settings, tasks, dependencies, and commit records
  (`src-tauri/src/storage/`).
  - The schema is versioned with `PRAGMA user_version` and migrated forward on startup;
    a database from a newer Toodo version is refused rather than modified.
  - Workspace and task IDs are never reused after a delete, because task IDs are part of
    branch and worktree names.
  - Writes that touch the task DAG run in one transaction: new edges are validated
    against the DAG rules, and `planned`/`ready` statuses are recomputed from the
    dependencies after every dependency, status, or task change. `planned` and `ready`
    cannot be set directly.
  - Every task status change appends a row to the task's status history in the same
    transaction (see [Task Timeline](#task-timeline)).
  - Deleting a workspace or task also deletes its tasks, dependency edges, commit
    records, and status history.

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
- Commits are recorded automatically: when it opens or updates a task's pull request,
  Toodo runs `git log <baseBranch>..<taskBranch>` in the worktree and stores the result. While a task
  is in progress, the user can refresh commit records manually.
- Storage uses SQLite through `rusqlite` (single-user local app; no async pool needed).
- Pull requests target GitHub only, through the `gh` CLI.
- Repositories inside WSL (`\\wsl$\...`) are not supported as local workspaces. Register
  them as SSH workspaces if needed.
- Task types are a fixed set (see [Task Types](#task-types)).
- Toodo is used for year-end performance reporting, so every task status change is
  timestamped and kept as history, and the dependency graph is drawn on a time axis
  (see [Timeline View](#timeline-view)).

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
- Correcting timeline timestamps by hand, for example to enter work done before the task
  was created in Toodo.
