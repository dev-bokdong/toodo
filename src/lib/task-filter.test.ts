import { describe, expect, it } from "vitest";
import type { Task } from "@/lib/bindings";
import { emptyTaskFilter, isFilterActive, matchesFilter } from "@/lib/task-filter";

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
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    issue: null,
    pullRequestMergedAt: null,
    pullRequest: null,
    resolution: null,
    completionNote: null,
    timeline: [],
    milestones: { plannedAt: null, startedAt: null, reviewAt: null, completedAt: null, pausedAt: null },
    ...overrides,
  };
}

const noTasks = new Map<number, Task>();

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

  it("narrows to one task type", () => {
    expect(matchesFilter(task({ taskType: "fix" }), { ...emptyTaskFilter, type: "fix" }, noTasks)).toBe(true);
    expect(matchesFilter(task(), { ...emptyTaskFilter, type: "fix" }, noTasks)).toBe(false);
  });

  it("counts a blank query as no filter", () => {
    expect(isFilterActive({ ...emptyTaskFilter, query: "   " })).toBe(false);
    expect(isFilterActive({ ...emptyTaskFilter, type: "docs" })).toBe(true);
  });
});
