---
name: toodo
description: Work on a Toodo task from its description to a pull request into the workspace base branch, splitting off prerequisite tasks when needed, and write task descriptions Toodo can track. Use when working in a Toodo task worktree or creating Toodo tasks.
---

# Working on a Toodo task

Toodo tracks your task and turns your pull request into the task's status: it moves the
task to `in_review` once a pull request from the task branch into the base branch exists,
and to `done` once that pull request is merged.

Never start, complete, or conclude your own task (the one `current_task` finds) with
`start_task`, `complete_task`, or `conclude_task`: it was started for you, and the merge
of its pull request completes it. Those tools are for other tasks, e.g. starting a ready
task for another agent session or concluding a duplicate. `conclude_task` never closes a
pull request; a task with an open one is concluded by the user in the Toodo app.
Deleting tasks happens in the Toodo app only.

## 1. Start

1. Call `current_task` with your absolute working directory. Note the task's `id`,
   `workspaceId`, `branch`, `baseBranch`, and `agentMerge` (whether you merge your pull
   request, see step 4 of "Otherwise: finish with a pull request").
2. If no task matches, you are not in a Toodo task worktree: tell the user and ask which
   task you are working on instead of guessing.
3. Read the description. `## Done when` is your finish line; `## Purpose` decides
   questions the criteria leave open. If the description is unclear or contradicts the
   code, call `report_blocker` with what you need and ask the user.
4. Before you plan, look up the history of the subject (see "Finding earlier work"
   below): earlier tasks may already have decided an approach, rejected one, or left
   work you build on.

## Finding earlier work

- `search_tasks` finds tasks of every workspace by words in their title, description,
  notes, completion note, branch, and pull request, best match first. Every word must
  appear (as a substring, in any language, short Korean words included); words are
  literal, never operators. Use two or three distinctive words, e.g. a component name
  and a verb, and search again with other words or synonyms before concluding there is
  nothing. Narrow with `workspace_id` or `status` when there are too many results.
- `get_task` returns one task with its whole history: description, status timeline,
  progress and blocker notes, completion note (for a task done as `not_adopted`, the
  conclusion of why it was not adopted), pull request, and commits, plus the tasks it
  `depends_on`, its `dependents`, and its `related` tasks. Read what was decided there
  before deciding again, and follow the links with more `get_task` calls when the
  history matters.
- Link tasks only by what they mean:
  - `add_dependency` only for a real prerequisite: work that has to be done before the
    other task can be finished (see step 3). It blocks the dependent task.
  - `add_related` for everything else worth reading together: an earlier task whose
    decisions this one builds on, a follow-up of a not adopted experiment, the same area
    in another workspace. Related links go both ways, cross workspaces, and never block
    starting a task. `remove_related` drops a wrong link.

## 2. Work

- Change only this worktree, on the task's branch.
- Call `add_progress_note` when you take a decision worth remembering (the choice and why)
  or finish a meaningful step.

## 3. A prerequisite is needed: plan it and stop

When you find work that has to land before this task can be finished and that is outside
its scope (a bug in the base branch, a missing API, a refactor the change depends on):

1. Call `search_tasks` for the work and reuse a matching open task instead of creating
   a duplicate; read relevant done tasks with `get_task` before planning the new ones.
2. Create each missing task with `create_task`, with a description in the format below.
   Do not make it depend on your task: your task could then never finish. Link it with
   `add_related` to earlier tasks it builds on that it does not have to wait for.
3. Tell the user which tasks you created and why this task needs them.
4. Call `add_dependency` once, with `task_id` = your task and `depends_on` = every
   prerequisite. This pauses your task.
5. End your turn. Do not continue the implementation and do not open a pull request; the
   user resumes the task in the Toodo app once its prerequisites are done.

## 4. Otherwise: finish with a pull request

1. Check every `## Done when` item and run the checks the repository uses (tests, type
   checks, linters).
2. Commit on the task branch and push it: `git push -u origin <branch>`.
3. Open the pull request into the workspace base branch:
   `gh pr create --base <baseBranch> --head <branch>`, titled with the task title, with
   a body that summarizes the change and lists the `## Done when` items. If a pull request
   for the branch already exists, push to it instead of opening another. A pull request
   into any other base is ignored by Toodo.
4. Merge it only if the workspace allows it. `current_task` returns `agentMerge`:
   - `never` (the default): stop there. Do not merge the pull request; the user merges
     it, and the merge completes the task.
   - `afterChecks`: wait until every check on the pull request has finished and passed
     (`gh pr checks <number> --watch`). When one fails, fix it, push, and wait again.
     Then merge the pull request into `baseBranch` (`gh pr merge <number> --merge`, or
     the merge method the repository allows); the merge completes the task.

   Toodo checks pull requests every minute, so it moves the task to `in_review` and,
   after the merge, to `done` within about a minute. Right after you merge, the task can
   still read `in_review`; that is expected. Do not call `complete_task` on it; read it
   again with `get_task` after a minute, and only if it is still `in_review` then, call
   `report_blocker` with what you see.

   Even with `afterChecks`, never merge while a check is failing or pending or when the
   pull request has no checks, never bypass branch protection (no `--admin`), never
   merge a pull request into any branch other than `baseBranch`, and never release, tag,
   or merge `baseBranch` onward (e.g. into `main`).

# Writing task descriptions

Every task you create gets a description in GitHub-flavored Markdown with these three
sections; Toodo renders it and uses it as the GitHub issue body.

```markdown
## Purpose

The outcome the task delivers and why it matters, in one to three sentences.

## Background

What exists today, what prompted the task (e.g. found while working on #12), related
tasks and what they decided, links, and constraints the implementer has to respect.

## Done when

- [ ] An observable, checkable result, e.g. `cargo test` covers the new rule
- [ ] ...
```

- Write it for an agent that starts from this description alone, without your session.
- Make every `## Done when` item verifiable, including the tests or checks expected.
- Use `-` lists, backticks for code, paths, and commands, fenced code blocks, and links;
  no raw HTML.
- Pick `task_type` by the kind of change (feat, fix, modify, refactor, docs, test, chore,
  rnd) and write a short imperative title.

# Adding a workspace

Every task belongs to a workspace, one repository on a machine. To plan tasks for a
repository Toodo does not have yet:

1. Call `list_workspaces` and reuse the workspace whose `location` already is the
   repository; never add a second workspace for the same path.
2. Only when it is missing and the user asked for it, call `create_workspace` with a
   `name` and the absolute `path` on the machine Toodo runs on. Pass `clone_url` to clone
   the repository into a new or empty directory, or leave it out to open a repository
   already cloned there. Set `default_base_branch` when pull requests go into a branch
   other than the repository's default one, e.g. `dev`.
3. Use the `id` it returns as `workspace_id` for `create_task`.

# Setting a machine's worktree root

Toodo creates each started task's worktree under the worktree root of the machine its
workspace is on, as `<root>/<workspace>/<type>-<title>-<id>`. `start_task` is rejected
for a task on a machine without one, e.g. on a freshly installed toodo-server.

1. Call `list_machine_settings`. It returns the machine Toodo runs on (`kind` `local`)
   and every known SSH machine (`kind` `ssh`, `label` = SSH host), each with its
   `worktreeRoot` (null when not set).
2. Set a missing root with `set_worktree_root`: `worktree_root` = an absolute directory
   on that machine, and `ssh_host` = the machine's label for an SSH machine (omit it for
   the local one). Use the directory the user named; when they named none, ask them
   instead of picking one.
3. Never change or clear a root that is already set unless the user asks; tasks started
   earlier keep their worktrees where they are.
