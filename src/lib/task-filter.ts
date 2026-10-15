import type { Task, TaskId, TaskType } from "@/lib/bindings";
import { dependenciesDone, isNotAdopted, isPaused } from "@/lib/task-graph";

/**
 * The status a task is shown with: its stored status, with paused and not adopted tasks told
 * apart, and a planned task that can be started now (every prerequisite done and adopted, or
 * none) shown as ready. A planned task still waiting on a prerequisite stays planned.
 */
export type ShownStatus = "planned" | "ready" | "paused" | "in_progress" | "in_review" | "done" | "not_adopted";

export const shownStatuses: ShownStatus[] = [
  "planned",
  "ready",
  "paused",
  "in_progress",
  "in_review",
  "done",
  "not_adopted",
];

/** `byId` holds the tasks of every workspace, since a prerequisite may belong to another one. */
export function shownStatus(task: Task, byId: ReadonlyMap<TaskId, Task>): ShownStatus {
  if (isNotAdopted(task)) {
    return "not_adopted";
  }
  if (isPaused(task)) {
    return "paused";
  }
  return task.status === "planned" && dependenciesDone(task, byId) ? "ready" : task.status;
}

/** What the task views are narrowed to; `null` lets every status or type through. */
export type TaskFilter = {
  /** Text searched for in the title and description, ignoring case. */
  query: string;
  status: ShownStatus | null;
  type: TaskType | null;
};

export const emptyTaskFilter: TaskFilter = { query: "", status: null, type: null };

export function isFilterActive(filter: TaskFilter): boolean {
  return filter.query.trim() !== "" || filter.status !== null || filter.type !== null;
}

/** Whether `task` passes `filter`: every word of the query appears in its title or description. */
export function matchesFilter(task: Task, filter: TaskFilter, byId: ReadonlyMap<TaskId, Task>): boolean {
  if (filter.status !== null && shownStatus(task, byId) !== filter.status) {
    return false;
  }
  if (filter.type !== null && task.taskType !== filter.type) {
    return false;
  }
  const text = `${task.title}\n${task.description}`.toLocaleLowerCase();
  return filter.query
    .toLocaleLowerCase()
    .split(/\s+/)
    .filter((word) => word !== "")
    .every((word) => text.includes(word));
}
