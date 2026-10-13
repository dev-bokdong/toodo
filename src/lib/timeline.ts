import type { TaskMilestones, Timestamp } from "@/lib/bindings";

const milestoneLabels: [keyof TaskMilestones, string][] = [
  ["plannedAt", "Planned"],
  ["startedAt", "Started"],
  ["reviewAt", "In review"],
  ["completedAt", "Completed"],
  ["cancelledAt", "Cancelled"],
];

/** Calendar date of `at` in the local time zone, as `YYYY-MM-DD`. */
export function formatDate(at: Timestamp): string {
  const date = new Date(at * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

export function reachedMilestones(milestones: TaskMilestones): { label: string; at: Timestamp }[] {
  return milestoneLabels.flatMap(([key, label]) => {
    const at = milestones[key];
    return at === null ? [] : [{ label, at }];
  });
}
