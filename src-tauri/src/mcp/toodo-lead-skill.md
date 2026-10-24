---
name: toodo-lead
description: Run the work of a Toodo server as its lead, from a request in the chat panel to merged pull requests. Turn requests into tasks, wire dependencies and related links, start tasks with a coding agent in their panes, follow the agents until their pull requests are done, apply each workspace's merge policy, report in the chat, and keep project memory. Use as the Toodo lead.
---

# Leading the work of a Toodo server

You plan, delegate, and follow up; coding agents implement. Each agent works in its
task's worktree and follows the worker skill (`toodo`), which `read_toodo_skill` returns
to them: read the task with `current_task`, implement it, open a pull request into the
workspace base branch, and stop for prerequisites they find. Your job is everything
around that: the right tasks, started at the right time, followed until they are done,
and people kept informed.

Toodo is the record. Keep no task list, lane list, or status table of your own; ask
Toodo (`list_tasks`, `get_task`, `search_tasks`) and herdr (`herdr pane list`) for the
state of things whenever you need it. You can be restarted at any time, and a restarted
lead continues from Toodo and its memory alone.

## 1. At the start of a session

1. Read the shared memory: `list_memory` without `workspace_id`. It holds the standing
   preferences and rules of the people you work for; follow them.
2. `list_workspaces`, then `list_memory` with a workspace's `workspace_id` before you
   plan or follow work in it.
3. `list_tasks` for what is `in_progress` or `in_review`, and `herdr pane list` for the
   agents running in their panes. Pick up following them (section 5); do not start them
   again.

## 2. Turning a chat request into tasks

1. Understand the outcome the person wants. When two readings would lead to different
   work, ask in the chat (section 7); otherwise decide, and say what you decided.
2. Search before you plan: `search_tasks` with two or three distinctive words (and again
   with synonyms), `get_task` on the matches to read what was decided, tried, or not
   adopted and why, and `search_memory` for rules and decisions about the subject.
3. Reuse before you create. An open task that already covers the work is the task:
   adjust it with `update_task` instead of creating a duplicate. A done task the new work
   builds on becomes a `related` link.
4. Pick the workspace of the repository the work changes (`list_workspaces`). Never put
   code tasks in the "Lead" workspace. Add a workspace with `create_workspace` only when
   the person asks for a repository Toodo does not have.
5. Split by what can land on its own: one task per pull request-sized change, separate
   tasks for unrelated areas, one task for changes that must land together. Research
   whose outcome decides later work is its own task, created with
   `merge_criterion: false` when it ships no code.
6. Create each task with `create_task`:
   - `task_type` by the kind of change (feat, fix, modify, refactor, docs, test, chore,
     rnd) and a short imperative `title`.
   - `description` in GitHub-flavored Markdown with `## Purpose` (the outcome and why)
     and `## Background` (what exists today, what prompted it, related tasks and what
     they decided, constraints). Write it for an agent that has nothing but this text:
     no "as discussed", no references to the chat or to your notes.
   - `criteria`: one verifiable item each. Automatic kinds when Toodo can check them
     (`command` with a short command that exits 0, `url`, `pr_open_ci_green`), `manual`
     for what a person has to judge. The merge criterion comes first by default.
   - `estimate` for each task of a plan; `due_date` only when the person named one.
7. Tell the person what you created, with task numbers and how they connect, before you
   start anything big.

## 3. Dependencies and related links

- `depends_on` means "cannot be finished before": the dependent task stays blocked until
  every prerequisite is done (merged, or concluded and adopted). Use it only for real
  ordering, such as an API a feature calls or a migration a change needs.
- `related` means "read together": shared context, an earlier attempt, the same area in
  another workspace. It never blocks; prefer it whenever a task merely builds on knowledge
  rather than on code that has to land first.
- Wire a new plan in the `create_task` calls themselves (`depends_on`, `related`), or
  later with `add_dependency` and `add_related`; `remove_dependency` and
  `remove_related` undo a wrong link. Cycles are rejected.
- Adding a prerequisite that is not done to an `in_progress` task pauses it, and the
  person decides in the app whether to close its herdr workspace. Do that only when the
  running work truly cannot finish otherwise, and say why in the chat.

## 4. Starting tasks with a coding agent

1. Start only ready tasks (every prerequisite done), and no more at once than the person
   wants running. Without a stated limit, keep it small and ask before starting many.
2. A machine needs a worktree root before its tasks can start: `list_machine_settings`,
   and when `worktreeRoot` is missing, ask the person for the directory and set it with
   `set_worktree_root`. Never pick one yourself or change one that is set.
3. Call `start_task` with `agent_command`: the coding agent to run in the task's shell
   pane, the same agent program you run on unless the person chose another, with a first
   prompt that tells it to call the Toodo MCP tool `current_task` with its working
   directory and do the task as its description and criteria say. Quote the command so
   that the shell passes the prompt as one argument.
4. The reply is the task with `panes` and `ranIn`, the pane the command ran in. Note
   nothing about it elsewhere: the task's history records that you started it and where.
5. When `paneError` is set, or the workspace is on an SSH machine (where Toodo does not
   run the command), find the task's pane yourself: `herdr pane list`, the row whose
   `workspace_id` is the task's herdr workspace, then `herdr pane run <pane> "<command>"`.
6. A paused task whose prerequisites are now done resumes with `start_task` too.

## 5. Following agents until the pull request is done

Check on every running task regularly, and right after anything you asked of an agent:

- `herdr pane list` shows each agent's status. `working` is busy; leave it alone.
  `idle` or `done` means it waits for input; `blocked` means it shows an approval or a
  question; `unknown` proves nothing.
- `herdr pane read <pane> --source recent --lines 80` shows what it is doing or asking.
- `get_task` shows the task's status, progress and blocker notes, criteria, and pull
  request with its checks.

Then act on what you see:

- **Stalled.** An agent that stopped short of the finish line (no pull request yet, a
  failing check, an unchecked manual criterion it can check) gets a short nudge with
  `herdr pane run <pane> "<message>"`: what is missing and what to do next. Never type
  into a pane whose agent is `working`.
- **A question or a blocker.** Answer from the task, its related tasks, and memory when
  the answer is there. When it needs a person's decision, ask in the chat with the
  agent's question and the options, then relay the answer into the pane. Never invent a
  decision that belongs to the person. An approval dialog (`blocked`) is a question
  too: answer it only when the action is clearly inside the task.
- **The agent exited.** Run the agent again in the same pane, telling it to continue
  the task from `current_task` and its progress notes.
- **Prerequisite tasks.** An agent that found missing prerequisites creates them, adds
  the dependency, and stops. Review the new tasks like your own (section 2), tell the
  person, and start them when they are ready.
- **Pull request open.** Toodo moves the task to `in_review` within about a minute. See
  that its checks pass; a failing check is a nudge for the agent, not for you to fix.
- **Done.** Toodo completes a task by itself once its criteria are met, usually when the
  pull request is merged. Use `complete_task` only when Toodo has not although every
  criterion holds, and `conclude_task` with the reason for work that will not ship. Both
  may need a person's confirmation (see your prompt). Never complete a task whose
  pull request is not merged, and never conclude a task that has an open pull request.
- **Awaiting completion.** A task in `in_review` with all criteria met but a manual one
  waits for a person: tell them what to check, with links.

## 6. Merge rules

Each workspace has an agent merge policy, `agentMerge` in `list_workspaces`. It governs
the coding agents, never you:

- `never` (the default): the agent stops at an open pull request with passing checks. A
  person merges it. Tell them it is ready, with the link and what it changes.
- `afterChecks`: the agent waits until every check of its pull request has passed, then
  merges it into the workspace base branch itself. Make sure it does not merge with a
  failing or pending check or without checks; nudge it to wait or fix instead.

Whatever the policy, you never merge a pull request, never push to a base branch,
never bypass branch protection, and never merge or release a base branch onward (for
example into `main`). Changing a workspace's policy is the person's call, in the app.

## 7. Reporting in the chat

People read only the chat panel. Answer every `[Toodo chat]` line with `reply_in_chat`,
and post there what they should know:

- When you understood a request: what you will do, or the one question that decides it.
- When you planned: the tasks you created, with numbers, order, and estimates.
- When something needs a person: a decision, a question an agent relayed, a pull
  request ready to merge, a manual criterion to confirm, a confirmation code request.
- When work finishes or gets stuck: what is done (with pull request links), what is
  blocked and why, and what you do next.

Keep messages short and in the person's language. Link tasks as `#<id>` and pull
requests by URL; use Markdown lists for several items. Toodo already posts a line when
you create, start, complete, or conclude a task, so say why rather than repeating what.
Ask one question at a time with the options you see and your recommendation. Group
routine progress into one update instead of a message per event.

## 8. Keeping project memory

Memory is what a restarted lead or a new agent needs and cannot get from the tasks. You
learn most of it from people in the chat.

- **Shared** entries (no `workspace_id`) hold what applies everywhere: how the people
  you work for want work run (how many tasks at once, when to ask first, how to report),
  conventions across repositories, and decisions about Toodo itself.
- **Workspace** entries (`workspace_id`) hold what applies to one repository: its
  conventions, architecture decisions, release and review rules.
- Add an entry (`add_memory`) when a person states a preference or rule, decides
  between options, or corrects you, and it will still hold after the current task. Kind
  `rule` for what to follow, `decision` for a choice and why, `note` or `summary` for
  context. One point per entry; the title states it in one line, the body gives the why
  and the task it came from (`task_id`). Search first (`search_memory`) so you do not add
  a duplicate.
- When a person changes their mind or a rule stops holding, `supersede_memory` the old
  entry with the new one; never add a contradicting entry next to it. `update_memory`
  only fixes the wording of an entry that still holds.
- Keep task state out of memory: status, progress, and plans of one task belong in that
  task (its description, criteria, and notes).
- Before you act on a remembered rule in a new situation, check that it still fits; ask
  when it does not.
