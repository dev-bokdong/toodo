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

export type TimelineScale = "day" | "week" | "month";

export const timelineScales: TimelineScale[] = ["day", "week", "month"];

export const pixelsPerDay: Record<TimelineScale, number> = { day: 48, week: 12, month: 3 };

/** Bars keep this width even for short tasks so their label stays readable. */
export const timelineBarMinWidth = 120;
/** Horizontal space kept free after a bar before another bar may share its lane. */
export const timelineBarGap = 16;

/** A half-open time range `[start, end)`, in Unix seconds. */
export type Period = { start: Timestamp; end: Timestamp };

export type TimelinePhase = "waiting" | "in_progress" | "in_review";

export type TimelineBar = {
  task: Task;
  lane: number;
  x: number;
  /** Width the bar occupies, at least `timelineBarMinWidth`. */
  width: number;
  /** Phases, positioned relative to `x` at true scale. */
  phases: { phase: TimelinePhase; x: number; width: number }[];
};

export type TimelineTick = { at: Timestamp; x: number; label: string };

export type TimelineLayout = {
  bars: TimelineBar[];
  /** Tasks without a known `plannedAt`, listed beside the timeline. */
  unplaced: Task[];
  lanes: number;
  start: Timestamp;
  end: Timestamp;
  width: number;
  ticks: TimelineTick[];
  /** Position of the current time, or null when it falls outside the range. */
  nowX: number | null;
};

const day = 24 * 60 * 60;

/**
 * The interval a task covers: from `plannedAt` to `completedAt` or `cancelledAt`; a task
 * still open extends to `now`. Null when `plannedAt` is unknown.
 */
export function taskInterval(task: Task, now: Timestamp): Period | null {
  const { plannedAt, readyAt, startedAt, reviewAt, completedAt, cancelledAt } = task.milestones;
  if (plannedAt === null) {
    return null;
  }
  const closed = task.status === "done" || task.status === "cancelled";
  // A closed task without its closing milestone ends at its last known one.
  const end =
    completedAt ??
    cancelledAt ??
    (closed ? Math.max(plannedAt, ...[readyAt, startedAt, reviewAt].filter((at) => at !== null)) : now);
  return { start: plannedAt, end: Math.max(end, plannedAt) };
}

function taskPhases(task: Task, { start, end }: Period): { phase: TimelinePhase; start: Timestamp; end: Timestamp }[] {
  const { startedAt, reviewAt } = task.milestones;
  const phases: { phase: TimelinePhase; start: Timestamp; end: Timestamp }[] = [
    { phase: "waiting", start, end: startedAt ?? reviewAt ?? end },
    { phase: "in_progress", start: startedAt ?? end, end: reviewAt ?? end },
    { phase: "in_review", start: reviewAt ?? end, end },
  ];
  return phases
    .map((phase) => ({ ...phase, start: Math.max(phase.start, start), end: Math.min(phase.end, end) }))
    .filter((phase) => phase.end > phase.start);
}

function startOfLocalDay(at: Timestamp): Date {
  const date = new Date(at * 1000);
  date.setHours(0, 0, 0, 0);
  return date;
}

const pad = (value: number) => String(value).padStart(2, "0");

/** Tick marks at local midnights: every day, every Monday, or every first of the month. */
function timelineTicks(start: Timestamp, end: Timestamp, scale: TimelineScale): { at: Timestamp; label: string }[] {
  const date = startOfLocalDay(start);
  if (scale === "week") {
    date.setDate(date.getDate() - ((date.getDay() + 6) % 7));
  } else if (scale === "month") {
    date.setDate(1);
  }
  const ticks: { at: Timestamp; label: string }[] = [];
  for (; date.getTime() / 1000 < end; ) {
    const at = date.getTime() / 1000;
    if (at >= start) {
      ticks.push({
        at,
        label:
          scale === "month"
            ? `${date.getFullYear()}-${pad(date.getMonth() + 1)}`
            : `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`,
      });
    }
    if (scale === "day") {
      date.setDate(date.getDate() + 1);
    } else if (scale === "week") {
      date.setDate(date.getDate() + 7);
    } else {
      date.setMonth(date.getMonth() + 1);
    }
  }
  return ticks;
}

/**
 * Lays tasks out on a time axis. Bars are placed in order of `plannedAt`, each in the first
 * lane that is free for its whole drawn width. With a `period`, only tasks overlapping it are
 * drawn, clipped to it; otherwise the axis runs from the first planned day to today.
 */
export function layoutTimeline(
  tasks: Task[],
  { now, scale, period }: { now: Timestamp; scale: TimelineScale; period: Period | null },
): TimelineLayout {
  const unplaced: Task[] = [];
  const placed: { task: Task; interval: Period }[] = [];
  for (const task of tasks) {
    const interval = taskInterval(task, now);
    if (interval === null) {
      unplaced.push(task);
    } else if (period === null || (interval.start < period.end && interval.end >= period.start)) {
      placed.push({ task, interval });
    }
  }
  placed.sort((a, b) => a.interval.start - b.interval.start || a.task.id - b.task.id);

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

  const laneEnds: number[] = [];
  const bars = placed.map(({ task, interval }) => {
    const clipped = { start: Math.max(interval.start, start), end: Math.min(interval.end, end) };
    const x = scaleX(clipped.start);
    const width = Math.max(scaleX(clipped.end) - x, timelineBarMinWidth);
    let lane = laneEnds.findIndex((laneEnd) => laneEnd <= x);
    if (lane === -1) {
      lane = laneEnds.length;
    }
    laneEnds[lane] = x + width + timelineBarGap;
    const phases = taskPhases(task, clipped).map((phase) => ({
      phase: phase.phase,
      x: scaleX(phase.start) - x,
      width: scaleX(phase.end) - scaleX(phase.start),
    }));
    return { task, lane, x, width, phases };
  });

  return {
    bars,
    unplaced,
    lanes: laneEnds.length,
    start,
    end,
    width: Math.max(scaleX(end), ...bars.map((bar) => bar.x + bar.width)),
    ticks: timelineTicks(start, end, scale).map((tick) => ({ ...tick, x: scaleX(tick.at) })),
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
