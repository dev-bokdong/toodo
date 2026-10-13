import { describe, expect, it } from "vitest";
import type { Task } from "@/lib/bindings";
import { canAddDependency, dependencyEdges, layoutTaskGraph } from "@/lib/task-graph";

function task(id: number, overrides: Partial<Task> = {}): Task {
  return {
    id,
    workspaceId: 1,
    taskType: "feat",
    title: `Task ${id}`,
    description: "",
    status: "ready",
    dependsOn: [],
    branch: null,
    worktreePath: null,
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    pullRequestMergedAt: null,
    timeline: [],
    milestones: {
      plannedAt: null,
      readyAt: null,
      startedAt: null,
      reviewAt: null,
      completedAt: null,
      cancelledAt: null,
    },
    ...overrides,
  };
}

describe("dependencyEdges", () => {
  it("draws edges from prerequisites and skips tasks outside the list", () => {
    const tasks = [task(1), task(2, { dependsOn: [1, 9] })];

    expect(dependencyEdges(tasks)).toEqual([{ id: "1->2", source: 1, target: 2 }]);
  });
});

describe("canAddDependency", () => {
  const tasks = [
    task(1, { status: "done" }),
    task(2, { status: "planned", dependsOn: [1] }),
    task(3, { status: "in_progress" }),
    task(4, { status: "cancelled" }),
    task(5),
  ];

  it("allows a new dependency for a planned or ready task", () => {
    expect(canAddDependency(tasks, 3, 2)).toBe(true);
    expect(canAddDependency(tasks, 1, 5)).toBe(true);
  });

  it("rejects self, duplicate, cancelled, and locked edges", () => {
    expect(canAddDependency(tasks, 5, 5)).toBe(false);
    expect(canAddDependency(tasks, 1, 2)).toBe(false);
    expect(canAddDependency(tasks, 4, 5)).toBe(false);
    expect(canAddDependency(tasks, 5, 3)).toBe(false);
    expect(canAddDependency(tasks, 5, 42)).toBe(false);
  });
});

describe("layoutTaskGraph", () => {
  it("places every task to the right of its prerequisites", async () => {
    const tasks = [task(1), task(2, { dependsOn: [1] }), task(3, { dependsOn: [2] }), task(4, { dependsOn: [1] })];

    const positions = await layoutTaskGraph(tasks);

    expect([...positions.keys()].sort()).toEqual([1, 2, 3, 4]);
    const x = (id: number) => positions.get(id)!.x;
    expect(x(1)).toBeLessThan(x(2));
    expect(x(2)).toBeLessThan(x(3));
    expect(x(1)).toBeLessThan(x(4));
  });
});
