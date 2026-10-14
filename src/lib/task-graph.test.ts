import { describe, expect, it } from "vitest";
import type { Task, TaskMilestones } from "@/lib/bindings";
import {
  canAddDependency,
  dependenciesDone,
  dependencyEdges,
  layoutTimeline,
  relatedTasks,
  timelineAxis,
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
    status: "planned",
    dependsOn: [],
    branch: null,
    worktreePath: null,
    herdrWorkspaceId: null,
    pullRequestUrl: null,
    pullRequestMergedAt: null,
    pullRequest: null,
    resolution: null,
    completionNote: null,
    timeline: [],
    milestones: {
      plannedAt: null,
      startedAt: null,
      reviewAt: null,
      completedAt: null,
      pausedAt: null,
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

describe("relatedTasks", () => {
  it("collects transitive prerequisites and dependents but not siblings", () => {
    // 1 -> 2 -> 3, 1 -> 4, 5 -> 3, 6 alone.
    const tasks = [
      task(1),
      task(2, { dependsOn: [1] }),
      task(3, { dependsOn: [2, 5] }),
      task(4, { dependsOn: [1] }),
      task(5),
      task(6),
    ];

    expect([...relatedTasks(tasks, 2)].sort()).toEqual([1, 2, 3]);
    expect([...relatedTasks(tasks, 1)].sort()).toEqual([1, 2, 3, 4]);
    expect([...relatedTasks(tasks, 6)]).toEqual([6]);
  });
});

describe("dependenciesDone", () => {
  it("waits for every prerequisite to be done and adopted", () => {
    const merged = task(1, { status: "done", resolution: "merged" });
    const notAdopted = task(2, { status: "done", resolution: "not_adopted" });
    const working = task(3, { status: "in_review" });
    const byId = new Map([merged, notAdopted, working].map((each) => [each.id, each]));

    expect(dependenciesDone(task(4, { dependsOn: [1] }), byId)).toBe(true);
    expect(dependenciesDone(task(5, { dependsOn: [1, 2] }), byId)).toBe(false);
    expect(dependenciesDone(task(6, { dependsOn: [3] }), byId)).toBe(false);
  });
});

describe("canAddDependency", () => {
  const tasks = [
    task(1, { status: "done" }),
    task(2, { status: "planned", dependsOn: [1] }),
    task(3, { status: "in_progress" }),
    task(4, { status: "in_review" }),
    task(5),
  ];

  it("allows a new dependency for a planned task", () => {
    expect(canAddDependency(tasks, 3, 2)).toBe(true);
    expect(canAddDependency(tasks, 1, 5)).toBe(true);
  });

  it("rejects self, duplicate, and locked edges", () => {
    expect(canAddDependency(tasks, 5, 5)).toBe(false);
    expect(canAddDependency(tasks, 1, 2)).toBe(false);
    expect(canAddDependency(tasks, 5, 4)).toBe(false);
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
  const waiting = task(3, { status: "planned", milestones: milestones({ plannedAt: jan(7) }) });
  const unknown = task(4, { status: "in_progress" });

  it("fixes a finished task from its start to its completion", () => {
    const layout = layoutTimeline([done], { now, scale: "day", period: null });

    expect(layout.start).toBe(jan(1));
    expect(layout.bars).toEqual([
      {
        task: done,
        row: 0,
        x: 2 * 48,
        width: 3 * 48,
        pending: false,
        phases: [
          { phase: "in_progress", x: 0, width: 2 * 48 },
          { phase: "in_review", x: 2 * 48, width: 48 },
        ],
        lead: { x: -2 * 48, planned: true, readyX: null },
      },
    ]);
    expect(layout.nowX).toBe(10 * 48);
    const axis = timelineAxis(layout, 0, layout.width);
    expect(axis.ticks.slice(0, 2)).toEqual([
      { at: jan(1), x: 0, label: "01-01" },
      { at: jan(2), x: 48, label: "01-02" },
    ]);
    expect(axis.majorTicks).toEqual([{ at: jan(1), x: 0, right: 31 * 48, label: "2026-01" }]);
  });

  it("ends a started task at now and puts a task not started at now", () => {
    const layout = layoutTimeline([waiting, unknown, open, done], { now, scale: "day", period: null });

    const bars = new Map(layout.bars.map((bar) => [bar.task.id, bar]));
    expect(layout.bars.map((bar) => [bar.task.id, bar.row])).toEqual([
      [1, 0],
      [2, 1],
      [3, 2],
    ]);
    expect(bars.get(2)).toMatchObject({ x: 3 * 48, width: 7 * 48, pending: false });
    expect(bars.get(2)!.lead).toEqual({ x: -1.5 * 48, planned: true, readyX: null });
    expect(bars.get(2)!.phases).toEqual([{ phase: "in_progress", x: 0, width: 7 * 48 }]);
    expect(bars.get(3)).toMatchObject({ x: layout.nowX, width: 120, pending: true, phases: [] });
    expect(layout.rows).toBe(3);
    expect(layout.unplaced).toEqual([unknown]);
  });

  it("fixes a paused task from its start to its pause", () => {
    const paused = task(5, {
      branch: "feat/task-5-5",
      milestones: milestones({ plannedAt: jan(1), startedAt: jan(3), pausedAt: jan(5) }),
    });

    const layout = layoutTimeline([paused], { now, scale: "day", period: null });

    expect(layout.bars[0]).toMatchObject({
      x: 2 * 48,
      width: 2 * 48,
      pending: false,
      phases: [{ phase: "in_progress", x: 0, width: 2 * 48 }],
    });
  });

  it("shows the pauses of a resumed task as phases between its stretches of work", () => {
    const resumed = task(6, {
      status: "done",
      branch: "feat/task-6-6",
      timeline: [
        { status: "planned", at: jan(1) },
        { status: "in_progress", at: jan(2) },
        { status: "planned", at: jan(3) },
        { status: "in_progress", at: jan(5) },
        { status: "in_review", at: jan(6) },
        { status: "done", at: jan(7) },
      ],
      milestones: milestones({ plannedAt: jan(1), startedAt: jan(2), reviewAt: jan(6), completedAt: jan(7) }),
    });

    const layout = layoutTimeline([resumed], { now, scale: "day", period: null });

    expect(layout.bars[0]).toMatchObject({ x: 48, width: 5 * 48 });
    expect(layout.bars[0].phases).toEqual([
      { phase: "in_progress", x: 0, width: 48 },
      { phase: "paused", x: 48, width: 2 * 48 },
      { phase: "in_progress", x: 3 * 48, width: 48 },
      { phase: "in_review", x: 4 * 48, width: 48 },
    ]);
  });

  it("moves bars that wait at now along with the current time", () => {
    const at = (time: number) => layoutTimeline([done, waiting], { now: time, scale: "hour", period: null });

    const before = at(jan(11, 9));
    const after = at(jan(11, 10));

    expect(after.nowX! - before.nowX!).toBeCloseTo(40);
    expect(before.bars[1].x).toBe(before.nowX);
    expect(after.bars[1].x).toBe(after.nowX);
    expect(after.bars[0]).toEqual(before.bars[0]);
    const axis = timelineAxis(after, 0, after.width);
    expect(axis.ticks.slice(0, 2).map((tick) => tick.label)).toEqual(["00:00", "01:00"]);
    expect(axis.majorTicks[0]).toEqual({ at: jan(1), x: 0, right: 960, label: "2026-01-01" });
  });

  it("draws ticks anywhere on an axis without a period", () => {
    const layout = layoutTimeline([done], { now, scale: "day", period: null });
    const dec = (day: number) => new Date(2025, 11, day).getTime() / 1000;

    const before = timelineAxis(layout, -2 * 48, 0);
    expect(before.ticks.map((tick) => tick.label)).toEqual(["12-30", "12-31"]);
    expect(before.majorTicks).toEqual([{ at: dec(1), x: -31 * 48, right: 0, label: "2025-12" }]);

    const after = timelineAxis(layout, 40 * 48, 41 * 48);
    expect(after.ticks.map((tick) => tick.label)).toEqual(["02-10"]);
    expect(after.majorTicks.map((tick) => tick.label)).toEqual(["2026-02"]);
  });

  it("draws the wait from the planned time through the completion of the last prerequisite", () => {
    const blocked = task(9, {
      dependsOn: [1],
      milestones: milestones({ plannedAt: jan(2) }),
    });

    const layout = layoutTimeline([done, blocked], { now, scale: "day", period: null });

    expect(layout.bars[1]).toMatchObject({ x: 10 * 48, pending: true });
    expect(layout.bars[1].lead).toEqual({ x: -9 * 48, planned: true, readyX: -5 * 48 });
  });

  it("orders dependents after their prerequisites and queues waiting ones behind them", () => {
    const first = task(6, { status: "planned" });
    const second = task(5, { status: "planned", dependsOn: [6] });
    const third = task(7, { dependsOn: [5, 1] });
    const afterDone = task(8, { dependsOn: [1] });

    const layout = layoutTimeline([third, second, afterDone, first, done], { now, scale: "day", period: null });

    expect(layout.bars.map((bar) => bar.task.id)).toEqual([1, 6, 5, 7, 8]);
    const x = new Map(layout.bars.map((bar) => [bar.task.id, bar.x]));
    expect(x.get(6)).toBe(layout.nowX);
    expect(x.get(5)).toBe(layout.nowX! + 120 + 16);
    expect(x.get(7)).toBe(layout.nowX! + 2 * (120 + 16));
    expect(x.get(8)).toBe(layout.nowX);
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
        startedAt: new Date(2025, 5, 1).getTime() / 1000,
        completedAt: new Date(2025, 5, 2).getTime() / 1000,
      }),
    });

    const layout = layoutTimeline([crossing, before], { now, scale: "month", period: yearPeriod(2026) });

    expect(layout.bars.map((bar) => bar.task.id)).toEqual([7]);
    expect(layout.bars[0].x).toBe(3);
    expect(layout.bars[0].phases).toEqual([{ phase: "in_progress", x: 0, width: 27 }]);
    expect(layout.bars[0].lead).toEqual({ x: -3, planned: false, readyX: null });
    const axis = timelineAxis(layout, -100, 2000);
    expect(axis.ticks.map((tick) => tick.label)).toEqual(
      Array.from({ length: 12 }, (_, month) => `2026-${String(month + 1).padStart(2, "0")}`),
    );
    expect(axis.majorTicks).toEqual([{ at: jan(1), x: 0, right: 365 * 3, label: "2026" }]);
  });
});

describe("timelineYears", () => {
  it("lists years from the first planned task to now, newest first", () => {
    const tasks = [task(1, { milestones: milestones({ plannedAt: new Date(2024, 6, 1).getTime() / 1000 }) }), task(2)];

    expect(timelineYears(tasks, jan(11))).toEqual([2026, 2025, 2024]);
    expect(timelineYears([], jan(11))).toEqual([2026]);
  });
});
