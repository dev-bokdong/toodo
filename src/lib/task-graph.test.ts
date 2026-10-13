import { describe, expect, it } from "vitest";
import type { Task, TaskMilestones } from "@/lib/bindings";
import {
  canAddDependency,
  dependencyEdges,
  layoutTimeline,
  timelineYears,
  yearPeriod,
} from "@/lib/task-graph";

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

function jan(day: number, hour = 0): number {
  return new Date(2026, 0, day, hour).getTime() / 1000;
}

function milestones(values: Partial<TaskMilestones>): TaskMilestones {
  return { ...task(0).milestones, ...values };
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

describe("layoutTimeline", () => {
  const now = jan(11);
  const done = task(1, {
    status: "done",
    milestones: milestones({ plannedAt: jan(1), startedAt: jan(3), reviewAt: jan(5), completedAt: jan(6) }),
  });
  const open = task(2, {
    status: "in_progress",
    milestones: milestones({ plannedAt: jan(2, 12), startedAt: jan(4) }),
  });
  const later = task(3, { status: "planned", milestones: milestones({ plannedAt: jan(7) }) });
  const unknown = task(4);

  it("splits a finished task into its phases on the time axis", () => {
    const layout = layoutTimeline([done], { now, scale: "day", period: null });

    expect(layout.start).toBe(jan(1));
    expect(layout.bars).toEqual([
      {
        task: done,
        lane: 0,
        x: 0,
        width: 5 * 48,
        phases: [
          { phase: "waiting", x: 0, width: 2 * 48 },
          { phase: "in_progress", x: 2 * 48, width: 2 * 48 },
          { phase: "in_review", x: 4 * 48, width: 48 },
        ],
      },
    ]);
    expect(layout.nowX).toBe(10 * 48);
    expect(layout.ticks.slice(0, 2)).toEqual([
      { at: jan(1), x: 0, label: "01-01" },
      { at: jan(2), x: 48, label: "01-02" },
    ]);
  });

  it("extends open tasks to now and packs bars into the first free lane", () => {
    const layout = layoutTimeline([later, unknown, open, done], { now, scale: "day", period: null });

    const bars = new Map(layout.bars.map((bar) => [bar.task.id, bar]));
    expect(layout.bars.map((bar) => bar.task.id)).toEqual([1, 2, 3]);
    expect(bars.get(2)).toMatchObject({ lane: 1, x: 1.5 * 48, width: 8.5 * 48 });
    expect(bars.get(2)!.phases.at(-1)).toEqual({ phase: "in_progress", x: 1.5 * 48, width: 7 * 48 });
    expect(bars.get(3)).toMatchObject({ lane: 0, x: 6 * 48, width: 4 * 48 });
    expect(layout.lanes).toBe(2);
    expect(layout.unplaced).toEqual([unknown]);
  });

  it("keeps short bars wide enough to read without overlapping in a lane", () => {
    const first = task(5, { status: "planned", milestones: milestones({ plannedAt: jan(10) }) });
    const second = task(6, { status: "planned", milestones: milestones({ plannedAt: jan(10, 12) }) });

    const layout = layoutTimeline([first, second], { now, scale: "week", period: null });

    expect(layout.bars.map((bar) => [bar.task.id, bar.lane, bar.width])).toEqual([
      [5, 0, 120],
      [6, 1, 120],
    ]);
  });

  it("narrows the axis to a period and clips the bars that cross it", () => {
    const crossing = task(7, {
      status: "in_progress",
      milestones: milestones({ plannedAt: new Date(2025, 11, 20).getTime() / 1000, startedAt: jan(2) }),
    });
    const before = task(8, {
      status: "done",
      milestones: milestones({
        plannedAt: new Date(2025, 5, 1).getTime() / 1000,
        completedAt: new Date(2025, 5, 2).getTime() / 1000,
      }),
    });

    const layout = layoutTimeline([crossing, before], { now, scale: "month", period: yearPeriod(2026) });

    expect(layout.bars.map((bar) => bar.task.id)).toEqual([7]);
    expect(layout.bars[0].x).toBe(0);
    expect(layout.bars[0].phases).toEqual([
      { phase: "waiting", x: 0, width: 3 },
      { phase: "in_progress", x: 3, width: 27 },
    ]);
    expect(layout.ticks.map((tick) => tick.label)).toEqual(
      Array.from({ length: 12 }, (_, month) => `2026-${String(month + 1).padStart(2, "0")}`),
    );
  });
});

describe("timelineYears", () => {
  it("lists years from the first planned task to now, newest first", () => {
    const tasks = [task(1, { milestones: milestones({ plannedAt: new Date(2024, 6, 1).getTime() / 1000 }) }), task(2)];

    expect(timelineYears(tasks, jan(11))).toEqual([2026, 2025, 2024]);
    expect(timelineYears([], jan(11))).toEqual([2026]);
  });
});
