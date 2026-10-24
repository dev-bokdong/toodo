import type { TaskEvent, TaskMilestones, Timestamp } from "@/lib/bindings";
import { statusNames } from "@/lib/status-styles";

const milestoneLabels: [keyof TaskMilestones, string][] = [
  ["plannedAt", "Planned"],
  ["startedAt", "Started"],
  ["reviewAt", statusNames.in_review],
  ["completedAt", "Completed"],
  ["pausedAt", statusNames.paused],
];

/** Calendar date of `at` in the local time zone, as `YYYY-MM-DD`. */
export function formatDate(at: Timestamp): string {
  const date = new Date(at * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export type WorkState = "in_progress" | "in_review" | "paused";

/** A stretch of a task's history in one state; `end` is null while the task is still in it. */
export type WorkSegment = { state: WorkState; start: Timestamp; end: Timestamp | null };

/**
 * The history of a task from its first start to its completion, oldest first: the stretches
 * it spent in progress, in review, and paused (back in `planned` after it started). Empty
 * when the history records no start.
 */
export function workSegments(timeline: TaskEvent[]): WorkSegment[] {
  const segments: WorkSegment[] = [];
  for (const event of timeline) {
    const last = segments.at(-1);
    if (last !== undefined && last.end === null) {
      last.end = event.at;
    }
    if (event.status === "done") {
      break;
    }
    if (event.status !== "planned") {
      segments.push({ state: event.status, start: event.at, end: null });
    } else if (segments.length > 0) {
      segments.push({ state: "paused", start: event.at, end: null });
    }
  }
  return segments;
}

/**
 * Seconds a task spent in progress or in review, leaving out its pauses; a stretch still
 * under way counts until `now`. Null when the history records no start.
 */
export function activeSeconds(timeline: TaskEvent[], now: Timestamp): number | null {
  const segments = workSegments(timeline);
  if (segments.length === 0) {
    return null;
  }
  return segments
    .filter((segment) => segment.state !== "paused")
    .reduce((total, segment) => total + Math.max((segment.end ?? now) - segment.start, 0), 0);
}

export function reachedMilestones(milestones: TaskMilestones): { label: string; at: Timestamp }[] {
  return milestoneLabels.flatMap(([key, label]) => {
    const at = milestones[key];
    return at === null ? [] : [{ label, at }];
  });
}
