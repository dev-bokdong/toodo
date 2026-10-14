import type { TaskType } from "@/lib/bindings";
import { cn } from "@/lib/utils";

// Outlined, unlike the filled status pills, and in hues the statuses do not use.
const taskTypeStyles: Record<TaskType, string> = {
  feat: "border-blue-300 text-blue-700 dark:border-blue-800 dark:text-blue-300",
  fix: "border-red-300 text-red-700 dark:border-red-800 dark:text-red-300",
  modify: "border-cyan-300 text-cyan-700 dark:border-cyan-800 dark:text-cyan-300",
  refactor: "border-indigo-300 text-indigo-700 dark:border-indigo-800 dark:text-indigo-300",
  docs: "border-slate-300 text-slate-700 dark:border-slate-700 dark:text-slate-300",
  test: "border-lime-400 text-lime-700 dark:border-lime-800 dark:text-lime-300",
  chore: "border-stone-300 text-stone-600 dark:border-stone-700 dark:text-stone-300",
  rnd: "border-fuchsia-300 text-fuchsia-700 dark:border-fuchsia-800 dark:text-fuchsia-300",
};

/** The type of a task, colored per type. */
export function TaskTypeBadge({ type, className }: { type: TaskType; className?: string }) {
  return (
    <span
      data-task-type={type}
      className={cn("shrink-0 rounded-md border px-1.5 font-mono text-xs", taskTypeStyles[type], className)}
    >
      {type}
    </span>
  );
}
