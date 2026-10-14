import { useState } from "react";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import type { ReviewEntry, ReviewOutcome } from "@/lib/bindings";
import { useWeekReview, useWorkspaces } from "@/lib/queries";
import { formatDate } from "@/lib/timeline";
import { cn } from "@/lib/utils";
import {
  addDays,
  commitSubject,
  groupByOutcome,
  outcomeLabels,
  reviewMarkdown,
  startOfWeek,
  weekPeriod,
} from "@/lib/week-review";

const outcomeStyles: Record<ReviewOutcome, string> = {
  completed: "text-emerald-700 dark:text-emerald-300",
  cancelled: "text-muted-foreground",
  opened_for_review: "text-violet-700 dark:text-violet-300",
  started: "text-amber-700 dark:text-amber-300",
  worked_on: "text-sky-700 dark:text-sky-300",
  planned: "text-muted-foreground",
};

export function WeekReview() {
  const [weekStart, setWeekStart] = useState(() => startOfWeek(new Date()));
  const [copyNotice, setCopyNotice] = useState<string | null>(null);
  const period = weekPeriod(weekStart);
  const review = useWeekReview(period.start, period.end);
  const workspaces = useWorkspaces();
  const names = new Map(workspaces.data?.map((workspace) => [workspace.id, workspace.name]));

  function moveTo(day: string) {
    setCopyNotice(null);
    setWeekStart(day);
  }

  async function copyMarkdown(entries: ReviewEntry[]) {
    try {
      await navigator.clipboard.writeText(reviewMarkdown(weekStart, entries, workspaces.data ?? []));
      setCopyNotice("Copied as Markdown.");
    } catch (error) {
      setCopyNotice(`Could not copy: ${String(error)}`);
    }
  }

  return (
    <section aria-labelledby="week-review-heading" className="flex flex-col gap-4">
      <header className="flex flex-col gap-1">
        <h2 id="week-review-heading" className="text-xl font-semibold tracking-tight">
          Week review
        </h2>
        <p className="text-sm text-muted-foreground">
          Work recorded from {weekStart} to {addDays(weekStart, 6)} across all workspaces.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <label htmlFor="week-start" className="text-xs text-muted-foreground">
          Week starting
        </label>
        <input
          id="week-start"
          type="date"
          required
          className={cn(inputClass, "w-40")}
          value={weekStart}
          onChange={(e) => e.target.value && moveTo(e.target.value)}
        />
        <Button size="sm" variant="outline" onClick={() => moveTo(addDays(weekStart, -7))}>
          Previous week
        </Button>
        <Button size="sm" variant="outline" onClick={() => moveTo(addDays(weekStart, 7))}>
          Next week
        </Button>
        <Button size="sm" variant="ghost" onClick={() => moveTo(startOfWeek(new Date()))}>
          This week
        </Button>
        <Button
          size="sm"
          className="ml-auto"
          disabled={!review.data}
          onClick={() => review.data && void copyMarkdown(review.data)}
        >
          Copy as Markdown
        </Button>
      </div>
      {copyNotice && <p className="text-xs text-muted-foreground">{copyNotice}</p>}

      {review.isPending && <p className="text-sm text-muted-foreground">Loading the review...</p>}
      {review.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load the review: {review.error.message}
        </p>
      )}
      {review.data?.length === 0 && (
        <p className="text-sm text-muted-foreground">No recorded activity in this week.</p>
      )}
      {review.data &&
        groupByOutcome(review.data).map((group) => (
          <section key={group.outcome} aria-label={outcomeLabels[group.outcome]} className="flex flex-col gap-2">
            <h3 className={cn("text-sm font-semibold", outcomeStyles[group.outcome])}>
              {outcomeLabels[group.outcome]}{" "}
              <span className="font-normal text-muted-foreground">({group.entries.length})</span>
            </h3>
            <ul className="flex flex-col divide-y rounded-lg border">
              {group.entries.map(({ task, at, commits }) => (
                <li key={task.id} className="flex flex-col gap-1 px-3 py-2">
                  <div className="flex items-center gap-2">
                    <span className="rounded-md border px-1.5 text-xs font-medium">{task.taskType}</span>
                    <span className="truncate text-sm font-medium">{task.title}</span>
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                      {names.get(task.workspaceId) ?? `Workspace ${task.workspaceId}`} &middot; {formatDate(at)}
                    </span>
                  </div>
                  {task.pullRequestUrl && (
                    <p className="truncate text-xs">
                      <span className="text-muted-foreground">Pull request </span>
                      <span className="select-all font-mono">{task.pullRequestUrl}</span>
                    </p>
                  )}
                  {commits.length > 0 && (
                    <ul aria-label={`Commits of #${task.id} this week`} className="flex flex-col gap-0.5">
                      {commits.map((commit) => (
                        <li key={commit.sha} className="flex gap-2 text-xs" title={commit.author}>
                          <code className="shrink-0 text-muted-foreground">{commit.sha.slice(0, 7)}</code>
                          <span className="truncate">{commitSubject(commit.message)}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
          </section>
        ))}
    </section>
  );
}
