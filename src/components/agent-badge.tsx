import type { AgentStatus, TaskId } from "@/lib/bindings";
import { cn } from "@/lib/utils";

const agentLabels: Record<AgentStatus, string> = {
  idle: "Agent idle",
  working: "Agent working",
  blocked: "Agent needs input",
  done: "Agent done",
};

const agentStyles: Record<AgentStatus, string> = {
  idle: "bg-muted text-muted-foreground",
  working: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200",
  blocked: "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
};

/** The status herdr reports for the agent working on task `taskId`. */
export function AgentBadge({
  taskId,
  status,
  className,
}: {
  taskId: TaskId;
  status: AgentStatus;
  className?: string;
}) {
  return (
    <span
      aria-label={`Agent of #${taskId}`}
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium",
        agentStyles[status],
        className,
      )}
    >
      <span aria-hidden className={cn("size-1.5 rounded-full bg-current", status === "working" && "animate-pulse")} />
      {agentLabels[status]}
    </span>
  );
}
