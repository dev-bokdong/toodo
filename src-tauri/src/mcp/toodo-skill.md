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
   `workspaceId`, `branch`, and `baseBranch`.
2. If no task matches, you are not in a Toodo task worktree: tell the user and ask which
   task you are working on instead of guessing.
3. Read the description. `## Done when` is your finish line; `## Purpose` decides
   questions the criteria leave open. If the description is unclear or contradicts the
   code, call `report_blocker` with what you need and ask the user.

## 2. Work

- Change only this worktree, on the task's branch.
- Call `add_progress_note` when you take a decision worth remembering (the choice and why)
  or finish a meaningful step.

## 3. A prerequisite is needed: plan it and stop

When you find work that has to land before this task can be finished and that is outside
its scope (a bug in the base branch, a missing API, a refactor the change depends on):

1. Call `list_tasks` for the workspace and reuse a matching task instead of creating a
   duplicate.
2. Create each missing task with `create_task`, with a description in the format below.
   Do not make it depend on your task: your task could then never finish.
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
4. Stop there. Do not merge the pull request; the merge completes the task.

# Writing task descriptions

Every task you create gets a description in GitHub-flavored Markdown with these three
sections; Toodo renders it and uses it as the GitHub issue body.

```markdown
## Purpose

The outcome the task delivers and why it matters, in one to three sentences.

## Background

What exists today, what prompted the task (e.g. found while working on #12), related
tasks, links, and constraints the implementer has to respect.

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
