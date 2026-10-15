import type { ChecksStatus, ReviewStatus, TaskId, TaskPullRequestStatus } from "@/lib/bindings";
import { cn } from "@/lib/utils";

const checksLabels: Record<ChecksStatus, string> = {
  passing: "CI passed",
  failing: "CI failed",
  pending: "CI running",
};

const checksStyles: Record<ChecksStatus, string> = {
  passing: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
  failing: "bg-rose-100 text-rose-800 dark:bg-rose-950 dark:text-rose-200",
  pending: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200",
};

const reviewLabels: Record<ReviewStatus, string> = {
  approved: "Approved",
  changes_requested: "Changes requested",
  pending: "Review pending",
};

const reviewStyles: Record<ReviewStatus, string> = {
  approved: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
  changes_requested: "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200",
  pending: "bg-muted text-muted-foreground",
};

const pillClass = "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium";

/** CI and review state of the pull request of task `taskId`; no CI badge when it has no checks. */
export function PullRequestBadges({ taskId, status }: { taskId: TaskId; status: TaskPullRequestStatus }) {
  return (
    <>
      {status.checks && (
        <span aria-label={`CI of #${taskId}`} className={cn(pillClass, checksStyles[status.checks])}>
          <span
            aria-hidden
            className={cn("size-1.5 rounded-full bg-current", status.checks === "pending" && "animate-pulse")}
          />
          {checksLabels[status.checks]}
        </span>
      )}
      <span aria-label={`Review of #${taskId}`} className={cn(pillClass, reviewStyles[status.review])}>
        {reviewLabels[status.review]}
      </span>
    </>
  );
}
