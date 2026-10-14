import type { Task, TaskId, TaskStatus, Timestamp } from "@/lib/bindings";

/** A dependency drawn from the prerequisite (`source`) to the task that waits on it (`target`). */
export type DependencyEdge = { id: string; source: TaskId; target: TaskId };

/** Statuses whose dependencies can still change; the backend rejects edits for the others. */
export const dependencyEditableStatuses: TaskStatus[] = ["planned", "ready"];

export function edgeId(source: TaskId, target: TaskId): string {
  return `${source}->${target}`;
}

/** Dependency edges among `tasks`; dependencies on tasks outside the list are skipped. */
export function dependencyEdges(tasks: Task[]): DependencyEdge[] {
  const ids = new Set(tasks.map((task) => task.id));
  return tasks.flatMap((task) =>
    task.dependsOn
      .filter((dependency) => ids.has(dependency))
      .map((dependency) => ({ id: edgeId(dependency, task.id), source: dependency, target: task.id })),
  );
}

/**
 * Whether `target` may gain a dependency on `source`. Cycles are left to the backend,
 * which owns the DAG rules and reports them as a command error.
 */
export function canAddDependency(tasks: Task[], source: TaskId, target: TaskId): boolean {
  if (source === target) {
    return false;
  }
  const from = tasks.find((task) => task.id === source);
  const to = tasks.find((task) => task.id === target);
  return (
    from !== undefined &&
    to !== undefined &&
    from.status !== "cancelled" &&
    dependencyEditableStatuses.includes(to.status) &&
    !to.dependsOn.includes(source)
  );
}

export type TimelineScale = "hour" | "day" | "week" | "month";

export const timelineScales: TimelineScale[] = ["hour", "day", "week", "month"];

export const pixelsPerDay: Record<TimelineScale, number> = { hour: 960, day: 48, week: 12, month: 3 };

/** Width of the bar of a task that has not started: it has no duration yet, only a place in line. */
export const timelinePendingWidth = 120;
/** Narrowest bar drawn for a task that has started, so a short task stays visible. */
export const timelineBarMinWidth = 4;
/** Horizontal space kept between a prerequisite's bar and a waiting dependent's bar. */
export const timelineBarGap = 16;

/** A half-open time range `[start, end)`, in Unix seconds. */
export type Period = { start: Timestamp; end: Timestamp };

export type TimelinePhase = "in_progress" | "in_review";

export type TimelineBar = {
  task: Task;
  /** Row of the bar; every task has its own row, prerequisites above their dependents. */
  row: number;
  x: number;
  width: number;
  /** The task has not started, so its bar waits at the current time (or after its prerequisites). */
  pending: boolean;
  /** Phases, positioned relative to `x` at true scale. */
  phases: { phase: TimelinePhase; x: number; width: number }[];
};

export type TimelineTick = { at: Timestamp; x: number; label: string };

export type TimelineLayout = {
  bars: TimelineBar[];
  /** Tasks that started or closed without a known time, listed beside the timeline. */
  unplaced: Task[];
  rows: number;
  start: Timestamp;
  end: Timestamp;
  width: number;
  /** Fine ticks of the axis header, also drawn as grid lines. */
  ticks: TimelineTick[];
  /** Coarse ticks of the axis header; the first one starts at the left edge of the axis. */
  majorTicks: TimelineTick[];
  /** Position of the current time, or null when it falls outside the range. */
  nowX: number | null;
};

const day = 24 * 60 * 60;

const pendingStatuses: TaskStatus[] = ["planned", "ready"];

/**
 * The interval a task's bar covers. A task that has not started sits at `now`; a started
 * task runs from `startedAt` to `now`; a closed task ends at `completedAt` or
 * `cancelledAt`. Null when a started or closed task lacks the milestones to place it.
 */
export function taskInterval(task: Task, now: Timestamp): Period | null {
  const { plannedAt, readyAt, startedAt, reviewAt, completedAt, cancelledAt } = task.milestones;
  if (pendingStatuses.includes(task.status)) {
    return { start: now, end: now };
  }
  const started = startedAt ?? reviewAt;
  if (task.status === "in_progress" || task.status === "in_review") {
    return started === null ? null : { start: Math.min(started, now), end: now };
  }
  // A closed task without its closing milestone ends at its last known one.
  const known = [plannedAt, readyAt, startedAt, reviewAt].filter((at): at is Timestamp => at !== null);
  const end = completedAt ?? cancelledAt ?? (known.length > 0 ? Math.max(...known) : null);
  if (end === null) {
    return null;
  }
  return { start: Math.min(started ?? end, end), end };
}

function taskPhases(task: Task, { start, end }: Period): { phase: TimelinePhase; start: Timestamp; end: Timestamp }[] {
  const { reviewAt } = task.milestones;
  const phases: { phase: TimelinePhase; start: Timestamp; end: Timestamp }[] = [
    { phase: "in_progress", start, end: reviewAt ?? end },
    { phase: "in_review", start: reviewAt ?? end, end },
  ];
  return phases
    .map((phase) => ({ ...phase, start: Math.max(phase.start, start), end: Math.min(phase.end, end) }))
    .filter((phase) => phase.end > phase.start);
}

type Placed = { task: Task; interval: Period };

/**
 * Orders tasks so that every task comes after the tasks it depends on; among the tasks
 * whose prerequisites are all placed, the earliest start (then the lowest id) goes first.
 */
function dependencyOrder(placed: Placed[]): Placed[] {
  const ids = new Set(placed.map(({ task }) => task.id));
  const remaining = new Map(placed.map(({ task }) => [task.id, task.dependsOn.filter((id) => ids.has(id)).length]));
  const compare = (a: Placed, b: Placed) => a.interval.start - b.interval.start || a.task.id - b.task.id;
  const done = new Set<TaskId>();
  const ordered: Placed[] = [];
  let available = placed.filter(({ task }) => remaining.get(task.id) === 0);
  while (ordered.length < placed.length) {
    if (available.length === 0) {
      // Only reachable with a cycle, which the backend rejects; fall back to time order.
      available = [placed.filter(({ task }) => !done.has(task.id)).sort(compare)[0]];
    }
    available.sort(compare);
    const next = available.shift()!;
    ordered.push(next);
    done.add(next.task.id);
    for (const item of placed) {
      if (!done.has(item.task.id) && item.task.dependsOn.includes(next.task.id)) {
        const left = remaining.get(item.task.id)! - 1;
        remaining.set(item.task.id, left);
        if (left === 0) {
          available.push(item);
        }
      }
    }
  }
  return ordered;
}

function startOfLocalDay(at: Timestamp): Date {
  const date = new Date(at * 1000);
  date.setHours(0, 0, 0, 0);
  return date;
}

const pad = (value: number) => String(value).padStart(2, "0");

type TickUnit = "hour" | "day" | "week" | "month" | "year";

const tickUnits: Record<TimelineScale, { minor: TickUnit; major: TickUnit }> = {
  hour: { minor: "hour", major: "day" },
  day: { minor: "day", major: "month" },
  week: { minor: "week", major: "month" },
  month: { minor: "month", major: "year" },
};

const minorLabels: Record<TickUnit, (date: Date) => string> = {
  hour: (date) => `${pad(date.getHours())}:00`,
  day: (date) => `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
  week: (date) => `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
  month: (date) => `${date.getFullYear()}-${pad(date.getMonth() + 1)}`,
  year: (date) => String(date.getFullYear()),
};

const majorLabels: Record<TickUnit, (date: Date) => string> = {
  ...minorLabels,
  day: (date) => `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
};

/**
 * Tick marks at the local starts of `unit` (hours, midnights, Mondays, firsts of the month,
 * or New Year). With `clampFirst`, a unit that began before `start` gets a tick at `start`.
 */
function timelineTicks(
  start: Timestamp,
  end: Timestamp,
  unit: TickUnit,
  label: (date: Date) => string,
  clampFirst: boolean,
): { at: Timestamp; label: string }[] {
  const date = new Date(start * 1000);
  date.setMinutes(0, 0, 0);
  if (unit !== "hour") {
    date.setHours(0);
  }
  if (unit === "week") {
    date.setDate(date.getDate() - ((date.getDay() + 6) % 7));
  } else if (unit === "month") {
    date.setDate(1);
  } else if (unit === "year") {
    date.setMonth(0, 1);
  }
  const ticks: { at: Timestamp; label: string }[] = [];
  for (; date.getTime() / 1000 < end; ) {
    const at = date.getTime() / 1000;
    if (at >= start) {
      ticks.push({ at, label: label(date) });
    } else if (clampFirst) {
      ticks.push({ at: start, label: label(date) });
    }
    if (unit === "hour") {
      date.setHours(date.getHours() + 1);
    } else if (unit === "day") {
      date.setDate(date.getDate() + 1);
    } else if (unit === "week") {
      date.setDate(date.getDate() + 7);
    } else if (unit === "month") {
      date.setMonth(date.getMonth() + 1);
    } else {
      date.setFullYear(date.getFullYear() + 1);
    }
  }
  return ticks;
}

/**
 * Lays tasks out as a Gantt chart: one row per task, prerequisites before their dependents.
 * A task that has not started waits at `now`, pushed right past the bars of its
 * prerequisites; a started task is fixed at `startedAt` and grows with `now`; a closed
 * task is fixed at both ends. With a `period`, only tasks overlapping it are drawn, clipped
 * to it; otherwise the axis runs from the first day with a bar to today.
 */
export function layoutTimeline(
  tasks: Task[],
  { now, scale, period }: { now: Timestamp; scale: TimelineScale; period: Period | null },
): TimelineLayout {
  const unplaced: Task[] = [];
  const placed: Placed[] = [];
  for (const task of tasks) {
    const interval = taskInterval(task, now);
    if (interval === null) {
      unplaced.push(task);
    } else if (period === null || (interval.start < period.end && interval.end >= period.start)) {
      placed.push({ task, interval });
    }
  }

  let start: Timestamp;
  let end: Timestamp;
  if (period !== null) {
    ({ start, end } = period);
  } else {
    start = startOfLocalDay(Math.min(now, ...placed.map(({ interval }) => interval.start))).getTime() / 1000;
    const last = Math.max(now, ...placed.map(({ interval }) => interval.end));
    const endDay = startOfLocalDay(last);
    endDay.setDate(endDay.getDate() + 1);
    end = endDay.getTime() / 1000;
  }
  const scaleX = (at: Timestamp) => ((at - start) / day) * pixelsPerDay[scale];

  const rightEdges = new Map<TaskId, number>();
  const bars = dependencyOrder(placed).map(({ task, interval }, row): TimelineBar => {
    const clipped = { start: Math.max(interval.start, start), end: Math.min(interval.end, end) };
    const pending = pendingStatuses.includes(task.status);
    let x: number;
    let width: number;
    let phases: TimelineBar["phases"] = [];
    if (pending) {
      const after = task.dependsOn.flatMap((id) => {
        const edge = rightEdges.get(id);
        return edge === undefined ? [] : [edge + timelineBarGap];
      });
      x = Math.max(scaleX(clipped.start), ...after);
      width = timelinePendingWidth;
    } else {
      x = scaleX(clipped.start);
      width = Math.max(scaleX(clipped.end) - x, timelineBarMinWidth);
      phases = taskPhases(task, clipped).map((phase) => ({
        phase: phase.phase,
        x: scaleX(phase.start) - x,
        width: scaleX(phase.end) - scaleX(phase.start),
      }));
    }
    rightEdges.set(task.id, x + width);
    return { task, row, x, width, pending, phases };
  });

  const { minor, major } = tickUnits[scale];
  const withX = (tick: { at: Timestamp; label: string }) => ({ ...tick, x: scaleX(tick.at) });
  return {
    bars,
    unplaced,
    rows: bars.length,
    start,
    end,
    // Room for the label of a narrow bar, which spills past its right end.
    width: Math.max(scaleX(end), ...bars.map((bar) => bar.x + Math.max(bar.width, timelinePendingWidth))),
    ticks: timelineTicks(start, end, minor, minorLabels[minor], false).map(withX),
    majorTicks: timelineTicks(start, end, major, majorLabels[major], true).map(withX),
    nowX: now >= start && now <= end ? scaleX(now) : null,
  };
}

export function yearPeriod(year: number): Period {
  return { start: new Date(year, 0, 1).getTime() / 1000, end: new Date(year + 1, 0, 1).getTime() / 1000 };
}

/** Calendar years from the first planned task to `now`, newest first. */
export function timelineYears(tasks: Task[], now: Timestamp): number[] {
  const last = new Date(now * 1000).getFullYear();
  const planned = tasks.flatMap((task) => (task.milestones.plannedAt === null ? [] : [task.milestones.plannedAt]));
  const first = new Date(Math.min(now, ...planned) * 1000).getFullYear();
  return Array.from({ length: last - first + 1 }, (_, index) => last - index);
}
