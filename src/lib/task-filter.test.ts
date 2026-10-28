import { describe, expect, it } from "vitest";
import type { Task } from "@/lib/bindings";
import { emptyTaskFilter, groupTasks, isFilterActive, matchesFilter } from "@/lib/task-filter";

function task(overrides: Partial<Task> = {}): Task {
  return {
    id: 1,
    workspaceId: 1,
    taskType: "feat",
    title: "Add storage",
    description: "",
    status: "planned",
    dependsOn: [],
    branch: null,
    worktreePath: null,
    worktreeMissing: false,
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    issue: null,
    estimate: null,
    dueDate: null,
    pullRequestMergedAt: null,
    pullRequest: null,
    mergedWith: null,
    resolution: null,
    completionNote: null,
    notes: [],
    criteria: [],
    timeline: [],
    milestones: { plannedAt: null, startedAt: null, reviewAt: null, completedAt: null, pausedAt: null },
    ...overrides,
  };
}

const noTasks = new Map<number, Task>();

describe("groupTasks", () => {
  it("puts tasks needing attention first, then active, ready, planned, and finished work", () => {
    const tasks = [
      task({ id: 1, status: "done", resolution: "merged" }),
      task({ id: 2, status: "done", resolution: "not_adopted" }),
      task({ id: 3, status: "planned" }),
      task({ id: 4, status: "planned", dependsOn: [6] }),
      task({ id: 5, status: "planned", branch: "feat/paused-5", dependsOn: [6] }),
      task({ id: 6, status: "in_progress" }),
      task({ id: 7, status: "in_review" }),
      task({ id: 8, status: "in_progress" }),
      task({ id: 9, status: "in_review" }),
    ];
    const byId = new Map(tasks.map((each) => [each.id, each]));
    const groups = groupTasks(tasks, byId, [9, 8]);
    expect(groups.map(({ group, tasks }) => [group, tasks.map((each) => each.id)])).toEqual([
      ["attention", [9, 8]],
      ["in_progress", [6]],
      ["in_review", [7]],
      ["ready", [3]],
      ["planned", [4, 5]],
      ["done", [1, 2]],
    ]);
  });

  it("leaves out empty sections", () => {
    expect(groupTasks([task()], noTasks, []).map(({ group }) => group)).toEqual(["ready"]);
  });
});

describe("matchesFilter", () => {
  it("finds every word of the query in the title or description, ignoring case", () => {
    const storage = task({ title: "Add SQLite storage", description: "Use rusqlite with migrations" });
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "sqlite" }, noTasks)).toBe(true);
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "  storage   MIGRATIONS " }, noTasks)).toBe(true);
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "storage timeline" }, noTasks)).toBe(false);
  });

  it("tells paused and not adopted tasks apart from their stored status", () => {
    const paused = task({ branch: "feat/add-storage-1" });
    const notAdopted = task({ status: "done", resolution: "not_adopted" });
    expect(matchesFilter(paused, { ...emptyTaskFilter, status: "paused" }, noTasks)).toBe(true);
    expect(matchesFilter(paused, { ...emptyTaskFilter, status: "planned" }, noTasks)).toBe(false);
    expect(matchesFilter(paused, { ...emptyTaskFilter, status: "ready" }, noTasks)).toBe(false);
    expect(matchesFilter(notAdopted, { ...emptyTaskFilter, status: "done" }, noTasks)).toBe(false);
    expect(matchesFilter(notAdopted, { ...emptyTaskFilter, status: "not_adopted" }, noTasks)).toBe(true);
  });

  it("tells planned tasks that can be started now apart from those waiting on a prerequisite", () => {
    const done = task({ id: 1, status: "done", resolution: "merged" });
    const working = task({ id: 2, status: "in_progress" });
    const notAdopted = task({ id: 3, status: "done", resolution: "not_adopted" });
    const byId = new Map([done, working, notAdopted].map((each) => [each.id, each]));
    const ready = { ...emptyTaskFilter, status: "ready" as const };
    const planned = { ...emptyTaskFilter, status: "planned" as const };

    expect(matchesFilter(task({ id: 4 }), ready, byId)).toBe(true);
    expect(matchesFilter(task({ id: 5, dependsOn: [1] }), ready, byId)).toBe(true);
    expect(matchesFilter(task({ id: 5, dependsOn: [1] }), planned, byId)).toBe(false);
    expect(matchesFilter(task({ id: 6, dependsOn: [1, 2] }), ready, byId)).toBe(false);
    expect(matchesFilter(task({ id: 6, dependsOn: [1, 2] }), planned, byId)).toBe(true);
    expect(matchesFilter(task({ id: 7, dependsOn: [3] }), ready, byId)).toBe(false);
    expect(matchesFilter(working, ready, byId)).toBe(false);
  });

  it("shows a task paused by a new prerequisite as ready once that prerequisite is done", () => {
    const milestones = { plannedAt: 10, startedAt: 20, reviewAt: null, completedAt: null, pausedAt: 30 };
    const pausedTask = (dependsOn: number[]) =>
      task({ id: 9, branch: "feat/add-storage-9", dependsOn, milestones });
    const doneAt = (id: number, at: number) =>
      task({ id, status: "done", resolution: "merged", milestones: { ...milestones, completedAt: at, pausedAt: null } });
    const byId = new Map([doneAt(1, 5), doneAt(2, 40), task({ id: 3, status: "in_progress" })].map((each) => [each.id, each]));
    const ready = { ...emptyTaskFilter, status: "ready" as const };
    const paused = { ...emptyTaskFilter, status: "paused" as const };

    // Prerequisite #2 was added while the task ran and completed after the pause.
    expect(matchesFilter(pausedTask([1, 2]), ready, byId)).toBe(true);
    expect(matchesFilter(pausedTask([1, 2]), paused, byId)).toBe(false);
    // Still waiting on #3.
    expect(matchesFilter(pausedTask([2, 3]), paused, byId)).toBe(true);
    // Paused by hand: its prerequisites were done before it started.
    expect(matchesFilter(pausedTask([1]), paused, byId)).toBe(true);
    expect(matchesFilter(pausedTask([]), paused, byId)).toBe(true);
  });

  it("narrows to one task type", () => {
    expect(matchesFilter(task({ taskType: "fix" }), { ...emptyTaskFilter, type: "fix" }, noTasks)).toBe(true);
    expect(matchesFilter(task(), { ...emptyTaskFilter, type: "fix" }, noTasks)).toBe(false);
  });

  it("counts a blank query as no filter", () => {
    expect(isFilterActive({ ...emptyTaskFilter, query: "   " })).toBe(false);
    expect(isFilterActive({ ...emptyTaskFilter, type: "docs" })).toBe(true);
  });
});
