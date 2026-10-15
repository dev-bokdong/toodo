import { useState } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { CompletionNote } from "@/components/task-list";
import { TaskTypeBadge } from "@/components/task-type-badge";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import type { ReviewEntry, ReviewOutcome, Workspace } from "@/lib/bindings";
import { useReview, useSaveReport, useWorkspaces } from "@/lib/queries";
import {
  commitSubject,
  durationKeys,
  durationLabels,
  formatChanges,
  formatDuration,
  groupByOutcome,
  outcomeLabels,
  periodBounds,
  periodContaining,
  periodLabel,
  periodNouns,
  periodTitles,
  pullRequestStats,
  reviewCsv,
  reviewFileName,
  reviewMarkdown,
  shiftPeriod,
  summarize,
  weekStarting,
  type PeriodKind,
  type ReviewPeriod,
} from "@/lib/review";
import { formatDate } from "@/lib/timeline";
import { cn } from "@/lib/utils";

const outcomeStyles: Record<ReviewOutcome, string> = {
  completed: "text-emerald-700 dark:text-emerald-300",
  opened_for_review: "text-violet-700 dark:text-violet-300",
  started: "text-amber-700 dark:text-amber-300",
  worked_on: "text-sky-700 dark:text-sky-300",
  planned: "text-muted-foreground",
};

const periodKinds: { kind: PeriodKind; label: string }[] = [
  { kind: "week", label: "Week" },
  { kind: "month", label: "Month" },
  { kind: "quarter", label: "Quarter" },
  { kind: "year", label: "Year" },
  { kind: "custom", label: "Custom" },
];

export function Review() {
  const [period, setPeriod] = useState<ReviewPeriod>(() => periodContaining("week", formatDate(Date.now() / 1000)));
  const [notice, setNotice] = useState<string | null>(null);
  const { start, end } = periodBounds(period);
  const review = useReview(start, end);
  const workspaces = useWorkspaces();
  const saveReport = useSaveReport();
  const names = new Map(workspaces.data?.map((workspace) => [workspace.id, workspace.name]));
  const noun = periodNouns[period.kind];

  function moveTo(next: ReviewPeriod) {
    setNotice(null);
    setPeriod(next);
  }

  function switchKind(kind: PeriodKind) {
    if (kind === period.kind) return;
    moveTo(kind === "custom" ? { ...period, kind } : periodContaining(kind, period.from));
  }

  async function copyMarkdown(entries: ReviewEntry[]) {
    try {
      await navigator.clipboard.writeText(reviewMarkdown(period, entries, workspaces.data ?? []));
      setNotice("Copied as Markdown.");
    } catch (error) {
      setNotice(`Could not copy: ${String(error)}`);
    }
  }

  async function saveAs(extension: "md" | "csv", entries: ReviewEntry[]) {
    setNotice(null);
    const workspaceList = workspaces.data ?? [];
    const contents =
      extension === "csv"
        ? reviewCsv(entries, workspaceList, Date.now() / 1000)
        : reviewMarkdown(period, entries, workspaceList);
    try {
      const path = await saveDialog({
        title: extension === "csv" ? "Save review as CSV" : "Save review as Markdown",
        defaultPath: reviewFileName(period, extension),
        filters: [extension === "csv" ? { name: "CSV", extensions: ["csv"] } : { name: "Markdown", extensions: ["md"] }],
      });
      if (typeof path !== "string") return;
      saveReport.mutate(
        { path, contents },
        {
          onSuccess: () => setNotice(`Saved to ${path}`),
          onError: (error) => setNotice(`Could not save: ${error.message}`),
        },
      );
    } catch (error) {
      setNotice(`Could not open the save dialog: ${String(error)}`);
    }
  }

  return (
    <section aria-labelledby="review-heading" className="flex flex-col gap-4">
      <header className="flex flex-col gap-1">
        <h2 id="review-heading" className="text-xl font-semibold tracking-tight">
          {periodTitles[period.kind]}
        </h2>
        <p className="text-sm text-muted-foreground">
          Work recorded from {period.from} to {period.to} across all workspaces.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <div role="group" aria-label="Review period" className="flex rounded-md border p-0.5">
          {periodKinds.map(({ kind, label }) => (
            <Button
              key={kind}
              size="sm"
              variant={period.kind === kind ? "secondary" : "ghost"}
              aria-pressed={period.kind === kind}
              onClick={() => switchKind(kind)}
            >
              {label}
            </Button>
          ))}
        </div>
        {period.kind === "week" && (
          <>
            <label htmlFor="week-start" className="text-xs text-muted-foreground">
              Week starting
            </label>
            <input
              id="week-start"
              type="date"
              required
              className={cn(inputClass, "w-40")}
              value={period.from}
              onChange={(e) => e.target.value && moveTo(weekStarting(e.target.value))}
            />
          </>
        )}
        {period.kind === "custom" && (
          <>
            <label htmlFor="review-from" className="text-xs text-muted-foreground">
              From
            </label>
            <input
              id="review-from"
              type="date"
              required
              max={period.to}
              className={cn(inputClass, "w-40")}
              value={period.from}
              onChange={(e) => e.target.value && e.target.value <= period.to && moveTo({ ...period, from: e.target.value })}
            />
            <label htmlFor="review-to" className="text-xs text-muted-foreground">
              To
            </label>
            <input
              id="review-to"
              type="date"
              required
              min={period.from}
              className={cn(inputClass, "w-40")}
              value={period.to}
              onChange={(e) => e.target.value && e.target.value >= period.from && moveTo({ ...period, to: e.target.value })}
            />
          </>
        )}
        {period.kind !== "week" && period.kind !== "custom" && (
          <span className="px-1 text-sm font-medium">{periodLabel(period)}</span>
        )}
        <Button size="sm" variant="outline" onClick={() => moveTo(shiftPeriod(period, -1))}>
          Previous {noun}
        </Button>
        <Button size="sm" variant="outline" onClick={() => moveTo(shiftPeriod(period, 1))}>
          Next {noun}
        </Button>
        {period.kind !== "custom" && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => moveTo(periodContaining(period.kind, formatDate(Date.now() / 1000)))}
          >
            This {noun}
          </Button>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" disabled={!review.data} onClick={() => review.data && void copyMarkdown(review.data)}>
          Copy as Markdown
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!review.data || saveReport.isPending}
          onClick={() => review.data && void saveAs("md", review.data)}
        >
          Save as Markdown
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={!review.data || saveReport.isPending}
          onClick={() => review.data && void saveAs("csv", review.data)}
        >
          Save as CSV
        </Button>
        {notice && <p className="text-xs text-muted-foreground">{notice}</p>}
      </div>

      {review.isPending && <p className="text-sm text-muted-foreground">Loading the review...</p>}
      {review.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load the review: {review.error.message}
        </p>
      )}
      {review.data?.length === 0 && (
        <p className="text-sm text-muted-foreground">No recorded activity in this {noun}.</p>
      )}
      {review.data && review.data.length > 0 && (
        <ReviewSummaryPanel period={period} entries={review.data} workspaces={workspaces.data ?? []} />
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
                    <TaskTypeBadge type={task.taskType} />
                    <span className="truncate text-sm font-medium">{task.title}</span>
                    {task.resolution === "not_adopted" && (
                      <span className="shrink-0 rounded-full bg-rose-100 px-2 py-0.5 text-xs font-medium text-rose-800 dark:bg-rose-950 dark:text-rose-200">
                        Not adopted
                      </span>
                    )}
                    <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                      {names.get(task.workspaceId) ?? `Workspace ${task.workspaceId}`} &middot; {formatDate(at)}
                    </span>
                  </div>
                  {task.pullRequestUrl && (
                    <p className="truncate text-xs">
                      <span className="text-muted-foreground">Pull request </span>
                      <span className="select-all font-mono">{task.pullRequestUrl}</span>
                      {task.pullRequest && (
                        <span className="text-muted-foreground"> &middot; {pullRequestStats(task.pullRequest)}</span>
                      )}
                    </p>
                  )}
                  {task.completionNote && (
                    <p aria-label={`Completion note of #${task.id}`} className="whitespace-pre-line text-xs">
                      {task.completionNote}
                    </p>
                  )}
                  {commits.length > 0 && (
                    <ul aria-label={`Commits of #${task.id} this ${noun}`} className="flex flex-col gap-0.5">
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

function ReviewSummaryPanel({
  period,
  entries,
  workspaces,
}: {
  period: ReviewPeriod;
  entries: ReviewEntry[];
  workspaces: Workspace[];
}) {
  const summary = summarize(period, entries, workspaces);
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const withoutNote = entries.filter(({ outcome, task }) => outcome === "completed" && task.completionNote === null);
  const busiestMonth = Math.max(1, ...(summary.completedPerMonth ?? []).map(({ count }) => count));
  return (
    <section aria-label="Summary" className="flex flex-col gap-3 rounded-lg border p-3">
      <dl className="flex flex-wrap gap-x-6 gap-y-2">
        {summary.outcomes.map(({ outcome, count }) => (
          <div key={outcome} className="flex flex-col">
            <dt className="text-xs text-muted-foreground">{outcomeLabels[outcome]}</dt>
            <dd className={cn("text-lg font-semibold", outcomeStyles[outcome])}>{count}</dd>
          </div>
        ))}
        {summary.notAdopted > 0 && (
          <div className="flex flex-col">
            <dt className="text-xs text-muted-foreground">Not adopted</dt>
            <dd className="text-lg font-semibold text-rose-700 dark:text-rose-300">{summary.notAdopted}</dd>
          </div>
        )}
        <div className="flex flex-col">
          <dt className="text-xs text-muted-foreground">Commits</dt>
          <dd className="text-lg font-semibold">{summary.commits}</dd>
        </div>
      </dl>
      {withoutNote.length > 0 && (
        <div className="flex flex-col gap-2 rounded-md bg-amber-100 px-2 py-1.5 text-xs text-amber-900 dark:bg-amber-950 dark:text-amber-200">
          <p role="status">
            {withoutNote.length === 1
              ? "1 completed task has no completion note."
              : `${withoutNote.length} completed tasks have no completion note.`}{" "}
            Add a note to each so the report says what the work achieved.
          </p>
          <ul aria-label="Completed tasks without a note" className="flex flex-col gap-1.5">
            {withoutNote.map(({ task }) => (
              <li key={task.id} className="flex flex-col gap-1 rounded-md bg-background px-2 py-1.5 text-foreground">
                <div className="flex min-w-0 items-center gap-2">
                  <TaskTypeBadge type={task.taskType} />
                  <span className="shrink-0 text-muted-foreground">#{task.id}</span>
                  <span className="truncate text-sm font-medium">{task.title}</span>
                  <span className="ml-auto shrink-0 text-muted-foreground">
                    {names.get(task.workspaceId) ?? `Workspace ${task.workspaceId}`}
                  </span>
                </div>
                <CompletionNote task={task} />
              </li>
            ))}
          </ul>
        </div>
      )}
      <dl className="grid gap-x-4 gap-y-1 text-sm sm:grid-cols-[auto_1fr]">
        {durationKeys.map((key) => {
          const stat = summary[key];
          return (
            <div key={key} className="contents">
              <dt className="text-muted-foreground">{durationLabels[key]}</dt>
              <dd>
                {stat === null
                  ? "No data"
                  : `${formatDuration(stat.median)} (${stat.tasks} ${stat.tasks === 1 ? "task" : "tasks"})`}
              </dd>
            </div>
          );
        })}
        {summary.changes && (
          <div className="contents">
            <dt className="text-muted-foreground">Changes in completed pull requests</dt>
            <dd>{formatChanges(summary.changes)}</dd>
          </div>
        )}
        {summary.completedByType.length > 0 && (
          <div className="contents">
            <dt className="text-muted-foreground">Completed by type</dt>
            <dd>{summary.completedByType.map(({ taskType, count }) => `${taskType} ${count}`).join(", ")}</dd>
          </div>
        )}
        {summary.completedByWorkspace.length > 0 && (
          <div className="contents">
            <dt className="text-muted-foreground">Completed by workspace</dt>
            <dd>{summary.completedByWorkspace.map(({ workspace, count }) => `${workspace} ${count}`).join(", ")}</dd>
          </div>
        )}
      </dl>
      {summary.completedPerMonth && (
        <div className="flex flex-col gap-1">
          <h3 className="text-xs text-muted-foreground">Completed per month</h3>
          <ul aria-label="Completed per month" className="flex flex-col gap-0.5">
            {summary.completedPerMonth.map(({ month, count }) => (
              <li key={month} className="flex items-center gap-2 text-xs">
                <span className="w-16 shrink-0 font-mono text-muted-foreground">{month}</span>
                <span
                  aria-hidden
                  className="h-2 rounded-sm bg-emerald-500/70"
                  style={{ width: `${(count / busiestMonth) * 60}%`, minWidth: count > 0 ? 4 : 0 }}
                />
                <span>{count}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
