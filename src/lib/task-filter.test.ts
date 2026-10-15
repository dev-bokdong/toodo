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

describe("matchesFilter", () => {
  it("finds every word of the query in the title or description, ignoring case", () => {
    const storage = task({ title: "Add SQLite storage", description: "Use rusqlite with migrations" });
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "sqlite" })).toBe(true);
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "  storage   MIGRATIONS " })).toBe(true);
    expect(matchesFilter(storage, { ...emptyTaskFilter, query: "storage timeline" })).toBe(false);
  });

  it("tells paused and not adopted tasks apart from their stored status", () => {
    const paused = task({ branch: "feat/add-storage-1" });
    const notAdopted = task({ status: "done", resolution: "not_adopted" });
    expect(matchesFilter(paused, { ...emptyTaskFilter, status: "paused" })).toBe(true);
    expect(matchesFilter(paused, { ...emptyTaskFilter, status: "planned" })).toBe(false);
    expect(matchesFilter(notAdopted, { ...emptyTaskFilter, status: "done" })).toBe(false);
    expect(matchesFilter(notAdopted, { ...emptyTaskFilter, status: "not_adopted" })).toBe(true);
  });

  it("narrows to one task type", () => {
    expect(matchesFilter(task({ taskType: "fix" }), { ...emptyTaskFilter, type: "fix" })).toBe(true);
    expect(matchesFilter(task(), { ...emptyTaskFilter, type: "fix" })).toBe(false);
  });

  it("counts a blank query as no filter", () => {
    expect(isFilterActive({ ...emptyTaskFilter, query: "   " })).toBe(false);
    expect(isFilterActive({ ...emptyTaskFilter, type: "docs" })).toBe(true);
  });
});
