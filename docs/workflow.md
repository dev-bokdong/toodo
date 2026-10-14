# Task Workflow

This document describes how a task moves from planning to a pull request.

## Task States

```
          Start / Resume          Open PR             merged
planned -----------------> in_progress ---------> in_review ---------> done
   ^                           |                      |
   +------------ Pause --------+----------------------+
```

| State | Meaning |
| --- | --- |
| `planned` | Nobody works on the task: it was not started yet, or it was paused. |
| `in_progress` | A worktree exists and implementation is happening in herdr. |
| `in_review` | The task branch is pushed and its pull request is open, waiting to be merged. |
| `done` | The pull request is merged; dependents can start from the updated base branch. |

Whether a `planned` task can be started is not a state: it can be started once every
task it depends on is `done` (it is **ready**), and its dependencies can change while it
is `planned`. The UI names a `planned` task that was worked on before "Paused" and
offers "Resume" instead of "Start".

Pausing ("Pause") an `in_progress` or `in_review` task moves it back to `planned`. It
keeps its branch, worktree, herdr workspace, commit records, and pull request, so
resuming it continues the same work (see step 3). `done` is a final state. A task that
is abandoned is deleted (step 8); a paused one stays in the reports.

Every state change is recorded with its time in the task's status history, which feeds
the timeline view and year-end reports (see
[architecture.md](architecture.md#task-timeline)).

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
else the checked-out branch. Deleting a workspace never deletes its repository, and is
rejected while a not-yet-started task of another workspace depends on one of its tasks.

### 2. Plan tasks

The user creates tasks and declares dependencies between them, also on tasks of other
workspaces (a library task that an application task needs first). Toodo rejects any
dependency that would create a cycle and shows which tasks are ready. The creation
time is recorded as the task's `plannedAt`. The sidebar's "All workspaces" entry lists
the tasks of every workspace on one timeline (its default view, which can be narrowed to
one workspace) and in one list.

### 3. Prepare the worktree

When the user starts a `planned` task whose dependencies are all `done`, Toodo:

1. Resolves the worktree directory as
   `<worktreeRoot>/<workspace-name>/<task-type>-<task-name>-<task-id>`, where
   `worktreeRoot` is set per machine (see
   [architecture.md](architecture.md#worktree-layout-and-branch-naming)). If the machine
   has no `worktreeRoot` yet, the user is asked to set it first.
2. Derives the task branch as `<task-type>/<task-name>-<task-id>`.
3. If the repository has no commit yet (e.g. a freshly created, empty GitHub repository)
   and no local `<baseBranch>`, points `HEAD` at `<baseBranch>`, creates an empty
   `Initial commit` on it (staged files stay staged), and pushes it to `origin` when that
   remote exists, so the base branch exists for the worktree and the pull request. If
   any of this fails, the task stays `planned`.
4. Calls `herdr worktree create --cwd <repository> --path <dir> --branch <branch>
   --base <baseBranch> --no-focus` (prefixed with `--machine <label>` for SSH
   workspaces) to create the git worktree and open it as a herdr workspace. If herdr
   fails, the task stays `planned` and nothing is stored.
5. Stores `branch`, `worktreePath`, and `herdrWorkspaceId` on the task and moves it to
   `in_progress`, which records `startedAt`.

A paused task that already has a branch resumes on it instead ("Resume"): Toodo opens its
kept worktree with `herdr worktree open --cwd <repository> --path <worktreePath>
--no-focus`, which also finds the herdr workspace that still has it open. If the
checkout is gone, the branch is checked out into a new worktree like "Restore worktree"
(step 4), or created from `<baseBranch>` when neither the repository nor `origin` has it.
`startedAt` stays the time of the first start.

### 4. Implement in herdr

Implementation happens directly in herdr: the user and/or AI agents work in panes bound
to the task's worktree. Toodo does not embed a terminal; it focuses the herdr workspace
(`herdr workspace focus`). Toodo may optionally send the task description as a prompt to
an agent (`herdr agent prompt`) and shows agent state (working, blocked, done) from
`pane.agent_status_changed` events. Tasks of local workspaces show the agent state of
their herdr workspace; if the user removes a task's worktree in herdr, Toodo forgets
the worktree, and if they open it again in herdr, Toodo follows the new workspace.

An `in_progress` or `in_review` task that lost its worktree this way can get it back
("Restore worktree"). Toodo checks out the task's branch, which herdr keeps when it
removes a worktree, with `herdr worktree create --cwd <repository> --path <dir>
--branch <branch> --base <branch> --no-focus`; herdr checks out an existing branch as
is and ignores `--base`. If the repository no longer has the branch, `--base
origin/<branch>` recreates it from `origin`; if neither exists, the restore is
rejected. The directory follows the current task title, and the status, commit
records, and pull request stay as they were.

### 5. Record commits

Toodo reads the commits on the task branch that are not on the base branch
(`git log --reverse <baseBranch>..<taskBranch>` in the worktree, over `ssh.exe` for remote
workspaces) and stores a `CommitRecord` (SHA, message, author, timestamp) for each one,
oldest first, replacing the previous records. This runs automatically when the task is
completed; while the task is in progress, the user can trigger it manually ("Refresh
commits") to refresh the list.

### 6. Open a pull request

When the user opens the pull request of an `in_progress` task ("Open PR"), Toodo:

1. Reads the commits of the task branch. A branch without commits ahead of the base
   branch is rejected, because there is nothing to open a pull request for, unless its
   pull request is already merged (step 3).
2. Reads the `origin` remote URL and derives the GitHub repository (`host/owner/name`)
   from it. A remote that is not a hosted repository URL is rejected.
3. Looks up the most recent pull request of the task branch in any state
   (`gh pr list --repo <repo> --head <branch> --state all`), which may have been opened
   outside Toodo, e.g. by an agent in herdr. An open or merged one whose base is not the
   workspace base branch is rejected, because dependent tasks start from that branch. A
   merged one is recorded without pushing (if the branch log is empty after the merge,
   the commit records stored earlier are kept) and the next step is skipped.
4. Pushes the task branch to `origin` (`git push --set-upstream origin <branch>`, over
   `ssh.exe` for remote workspaces), then reuses the open pull request, or, when there
   is none or the last one was closed, opens one with
   `gh pr create --repo <repo> --head <branch> --base <baseBranch>`, titled with the task
   title and described with the task description. `gh` always runs on this machine, so
   SSH machines do not need it.
5. In one transaction, stores the commit records and the pull request URL (and, for a
   merged one, its `mergedAt` as `pullRequestMergedAt`) and moves the task to
   `in_review`, which records `reviewAt`. Dependent tasks keep waiting: their worktrees start from the base
   branch, which does not contain this work until the pull request is merged.

If any step fails, the task keeps its status and previous commit records and the error is
shown; opening it again is safe, because pushing is repeatable and an open pull request
is reused instead of created twice. The same action on an `in_review` task ("Push to
PR") pushes commits added after review feedback and refreshes the commit records.

Review and merging happen on GitHub; Toodo never merges a pull request itself.

### 7. Complete the task

When the user completes an `in_review` task, Toodo reads the state, base branch, and
merge time of its pull request (`gh pr view <url> --json url,state,baseRefName,mergedAt`). A pull request into
another branch than the workspace base branch is rejected. Only a merged pull request
completes the task. Toodo first reads what reviews need about it (`gh pr view <url> --json
number,title,additions,deletions,changedFiles,mergeCommit,author,reviews,commits`); a
failure keeps the task `in_review`, so a completed task always has this snapshot. It then
closes its herdr workspace (`herdr workspace close
<herdrWorkspaceId>`), which keeps the worktree checkout on disk, then moves the task to
`done` (recording `completedAt`), stores `pullRequestMergedAt` and the pull request
snapshot, replaces the commit records with the commits of the pull request (so commits
pushed after it was opened are recorded too; a pull request without commits keeps the
records stored earlier), and clears `herdrWorkspaceId` in one transaction, which may make
dependent tasks ready to start. A workspace herdr no longer has is treated as already closed; any other herdr
failure keeps the task `in_review`, and completing it again is safe. An open pull request is rejected until it is
merged; a pull request closed without merging is rejected too, so the user reopens it or
pauses the task.

The user rarely needs to complete a task by hand: while the app runs, Toodo checks the
pull request of every `in_review` task once a minute and completes each task whose pull
request was merged, in the same way and with the same checks. Open and closed pull
requests are left alone. A failed check (e.g. `gh` is not signed in, or a herdr failure)
keeps the task `in_review` and is logged once until it changes; the next check tries
again. The task list refreshes when a task is completed this way.

In the same check, Toodo also looks up the pull request of the branch of every
`in_progress` task that has a branch and a worktree (steps 2 and 3 of opening a pull
request, `gh pr list --repo <repo> --head <branch> --state all`), so a pull request
opened outside Toodo, e.g. by an agent in herdr, is picked up within a minute. An open
or merged one into the workspace base branch is recorded as in step 5 without pushing,
which moves the task to `in_review`; a merged one then completes the task in the same
check. A branch without a pull request, or with a closed one only, leaves the task
`in_progress`; a pull request into another base branch or a failed lookup is logged once
until it changes.

Completion never waits for the user: dependents start as soon as the work is on the base
branch, and `completedAt` stays the time of the merge check. Instead, a `done` task
without a completion note is marked "Needs note" in the task list, where the user writes
what the work achieved (the `set_completion_note` command; only `done` tasks have one,
and an empty note clears it). The review counts completed tasks without a note so they
can be filled in before a report is written.

The worktree stays until the task is deleted (step 8).

### 8. Delete the task and its worktree

When the user deletes a task that is not `in_progress` or `in_review` and still has a
worktree ("Delete"), Toodo first removes the worktree:

1. If the task still has a herdr workspace (a paused task), calls
   `herdr worktree remove --workspace <herdrWorkspaceId>` (prefixed with
   `--machine <profile-id>` for SSH workspaces), which deletes the checkout and closes
   its herdr workspace. The task branch is kept, locally and on `origin`.
2. If the task has no herdr workspace (a completed task, whose workspace was closed) or
   herdr no longer has it (the user closed it, which keeps the checkout, or herdr lost
   its session), opens the checkout again with
   `herdr worktree open --cwd <repository> --path <worktreePath> --no-focus` and removes
   it through the new workspace. If herdr cannot find the checkout either, it is already
   gone and nothing is removed.
3. Clears `worktreePath` and `herdrWorkspaceId` on the task, then deletes the task with
   its dependency edges, commit records, and status history.

A checkout with modified or untracked files is rejected and kept together with the task;
the user can then force the delete ("Force delete", `--force`), which discards those
files. Any other herdr failure keeps the task unchanged. A task without a worktree is
deleted without running herdr.
