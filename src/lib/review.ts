import type {
  PullRequestSnapshot,
  ReviewEntry,
  ReviewOutcome,
  Task,
  TaskResolution,
  TaskType,
  Timestamp,
  Workspace,
} from "@/lib/bindings";
import { activeSeconds, formatDate } from "@/lib/timeline";

export const outcomeLabels: Record<ReviewOutcome, string> = {
  completed: "Completed",
  opened_for_review: "Opened for review",
  started: "Started",
  worked_on: "Worked on",
  planned: "Planned",
};

const outcomeOrder = Object.keys(outcomeLabels) as ReviewOutcome[];

export type PeriodKind = "week" | "month" | "quarter" | "year" | "custom";

/** A reviewed period: the local calendar days `from` to `to`, both included, as `YYYY-MM-DD`. */
export interface ReviewPeriod {
  kind: PeriodKind;
  from: string;
  to: string;
}

export const periodNouns: Record<PeriodKind, string> = {
  week: "week",
  month: "month",
  quarter: "quarter",
  year: "year",
  custom: "period",
};

export const periodTitles: Record<PeriodKind, string> = {
  week: "Week review",
  month: "Month review",
  quarter: "Quarter review",
  year: "Year review",
  custom: "Review",
};

function parseDay(day: string): [number, number, number] {
  const [year, month, date] = day.split("-").map(Number);
  return [year, month - 1, date];
}

function localMidnight(day: string, days = 0): Date {
  const [year, month, date] = parseDay(day);
  return new Date(year, month, date + days);
}

function dayOf(date: Date): string {
  return formatDate(date.getTime() / 1000);
}

/** The Monday of the week containing `date`, as `YYYY-MM-DD` in the local time zone. */
export function startOfWeek(date: Date): string {
  const sinceMonday = (date.getDay() + 6) % 7;
  return dayOf(new Date(date.getFullYear(), date.getMonth(), date.getDate() - sinceMonday));
}

export function addDays(day: string, days: number): string {
  return dayOf(localMidnight(day, days));
}

export function weekStarting(day: string): ReviewPeriod {
  return { kind: "week", from: day, to: addDays(day, 6) };
}

const monthsPerPeriod = { month: 1, quarter: 3, year: 12 } as const;

function monthsPeriod(kind: "month" | "quarter" | "year", year: number, firstMonth: number): ReviewPeriod {
  const months = monthsPerPeriod[kind];
  return {
    kind,
    from: dayOf(new Date(year, firstMonth, 1)),
    to: dayOf(new Date(year, firstMonth + months, 0)),
  };
}

/**
 * The week (from its Monday), month, quarter, or year containing the day `day`. A custom
 * period becomes the single day.
 */
export function periodContaining(kind: PeriodKind, day: string): ReviewPeriod {
  const [year, month] = parseDay(day);
  switch (kind) {
    case "week":
      return weekStarting(startOfWeek(localMidnight(day)));
    case "month":
      return monthsPeriod(kind, year, month);
    case "quarter":
      return monthsPeriod(kind, year, month - (month % 3));
    case "year":
      return monthsPeriod(kind, year, 0);
    case "custom":
      return { kind, from: day, to: day };
  }
}

/** The period `steps` periods later (earlier when negative); a custom period moves by its length. */
export function shiftPeriod(period: ReviewPeriod, steps: number): ReviewPeriod {
  switch (period.kind) {
    case "week":
      return weekStarting(addDays(period.from, 7 * steps));
    case "month":
    case "quarter":
    case "year": {
      const [year, month] = parseDay(period.from);
      return monthsPeriod(period.kind, year, month + monthsPerPeriod[period.kind] * steps);
    }
    case "custom": {
      const length = periodDays(period);
      return { kind: "custom", from: addDays(period.from, length * steps), to: addDays(period.to, length * steps) };
    }
  }
}

function periodDays(period: ReviewPeriod): number {
  return Math.round((localMidnight(period.to, 1).getTime() - localMidnight(period.from).getTime()) / 86_400_000);
}

/**
 * The period as `[start, end)` in seconds, from local midnight of `from` to local midnight
 * after `to`, so a period with a daylight saving change is an hour shorter or longer.
 */
export function periodBounds(period: ReviewPeriod): { start: Timestamp; end: Timestamp } {
  return {
    start: localMidnight(period.from).getTime() / 1000,
    end: localMidnight(period.to, 1).getTime() / 1000,
  };
}

export function periodLabel(period: ReviewPeriod): string {
  const [year, month] = parseDay(period.from);
  switch (period.kind) {
    case "month":
      return period.from.slice(0, 7);
    case "quarter":
      return `${year} Q${Math.floor(month / 3) + 1}`;
    case "year":
      return String(year);
    case "week":
    case "custom":
      return `${period.from} to ${period.to}`;
  }
}

/** Entries grouped by outcome, most advanced first; outcomes without entries are left out. */
export function groupByOutcome(entries: ReviewEntry[]): { outcome: ReviewOutcome; entries: ReviewEntry[] }[] {
  return outcomeOrder.flatMap((outcome) => {
    const matching = entries.filter((entry) => entry.outcome === outcome);
    return matching.length === 0 ? [] : [{ outcome, entries: matching }];
  });
}

export function commitSubject(message: string): string {
  return message.split("\n", 1)[0];
}

function plural(count: number, noun: string): string {
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}

/** The number, size, and reviews of a merged pull request, e.g. `#12, +10/-3 in 2 files, 1 review`. */
export function pullRequestStats(pullRequest: PullRequestSnapshot): string {
  return `#${pullRequest.number}, +${pullRequest.additions}/-${pullRequest.deletions} in ${plural(
    pullRequest.changedFiles,
    "file",
  )}, ${plural(pullRequest.reviewCount, "review")}`;
}

/** A completion note on one line, for list items and CSV cells. */
function oneLine(note: string): string {
  return note.split(/\s*\n\s*/).join(" ");
}

/** Size of the merged pull requests of completed tasks, summed. */
export interface ChangeStat {
  pullRequests: number;
  additions: number;
  deletions: number;
  changedFiles: number;
}

/** The median of durations in seconds over the tasks that reached both milestones. */
export interface DurationStat {
  median: number;
  tasks: number;
}

export interface ReviewSummary {
  outcomes: { outcome: ReviewOutcome; count: number }[];
  commits: number;
  completedByType: { taskType: TaskType; count: number }[];
  completedByWorkspace: { workspace: string; count: number }[];
  /** Planned to completed. */
  leadTime: DurationStat | null;
  /** Started to completed. */
  cycleTime: DurationStat | null;
  /** Time spent in progress or in review, leaving out pauses. */
  activeTime: DurationStat | null;
  /** Opened for review to merged, or to completed when the merge time is unknown. */
  reviewTime: DurationStat | null;
  /** Tasks completed in each calendar month of the period; `null` for a period within one month. */
  completedPerMonth: { month: string; count: number }[] | null;
  /** Merged pull requests of completed tasks; `null` when none was recorded. */
  changes: ChangeStat | null;
  /** Completed tasks the user has not written a completion note for. */
  withoutNote: number;
  /** Completed tasks that were concluded without merging, e.g. research that was not adopted. */
  notAdopted: number;
}

/** How a done task ended, for reports; null for a task that is not done. */
export function resolutionLabel(task: { resolution: TaskResolution | null }): string | null {
  switch (task.resolution) {
    case "merged":
      return "Merged";
    case "not_adopted":
      return "Not adopted";
    case null:
      return null;
  }
}

function median(values: number[]): DurationStat | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  const value = sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
  return { median: value, tasks: sorted.length };
}

function countBy<K extends string>(keys: K[]): { key: K; count: number }[] {
  const counts = new Map<K, number>();
  for (const key of keys) counts.set(key, (counts.get(key) ?? 0) + 1);
  return [...counts]
    .map(([key, count]) => ({ key, count }))
    .sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
}

function durations(entries: ReviewEntry[], span: (entry: ReviewEntry) => [Timestamp | null, Timestamp | null]) {
  return entries.flatMap((entry) => {
    const [from, to] = span(entry);
    return from === null || to === null ? [] : [to - from];
  });
}

/** Counts and durations of the reviewed period, from the recorded milestones of its entries. */
export function summarize(period: ReviewPeriod, entries: ReviewEntry[], workspaces: Workspace[]): ReviewSummary {
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const completed = entries.filter((entry) => entry.outcome === "completed");
  const months = periodMonths(period);
  const pullRequests = completed.flatMap(({ task }) => (task.pullRequest ? [task.pullRequest] : []));
  return {
    outcomes: groupByOutcome(entries).map((group) => ({ outcome: group.outcome, count: group.entries.length })),
    commits: entries.reduce((total, entry) => total + entry.commits.length, 0),
    completedByType: countBy(completed.map((entry) => entry.task.taskType)).map(({ key, count }) => ({
      taskType: key,
      count,
    })),
    completedByWorkspace: countBy(
      completed.map((entry) => names.get(entry.task.workspaceId) ?? `workspace ${entry.task.workspaceId}`),
    ).map(({ key, count }) => ({ workspace: key, count })),
    leadTime: median(durations(completed, ({ task, at }) => [task.milestones.plannedAt, at])),
    cycleTime: median(durations(completed, ({ task, at }) => [task.milestones.startedAt, at])),
    activeTime: median(
      completed.flatMap(({ task, at }) => {
        const active = activeSeconds(task.timeline, at);
        return active === null ? [] : [active];
      }),
    ),
    reviewTime: median(
      durations(completed, ({ task, at }) => [task.milestones.reviewAt, task.pullRequestMergedAt ?? at]),
    ),
    completedPerMonth:
      months.length < 2
        ? null
        : months.map((month) => ({
            month,
            count: completed.filter((entry) => formatDate(entry.at).startsWith(month)).length,
          })),
    changes:
      pullRequests.length === 0
        ? null
        : pullRequests.reduce(
            (total, pullRequest) => ({
              pullRequests: total.pullRequests + 1,
              additions: total.additions + pullRequest.additions,
              deletions: total.deletions + pullRequest.deletions,
              changedFiles: total.changedFiles + pullRequest.changedFiles,
            }),
            { pullRequests: 0, additions: 0, deletions: 0, changedFiles: 0 },
          ),
    withoutNote: completed.filter(({ task }) => task.completionNote === null).length,
    notAdopted: completed.filter(({ task }) => task.resolution === "not_adopted").length,
  };
}

/** Merged pull requests of completed tasks with their size and reviews, summed. */
export interface PullRequestTotals extends ChangeStat {
  /** Submitted reviews by anyone but the author, over all pull requests. */
  reviews: number;
  /** Pull requests with at least one such review. */
  reviewed: number;
}

/** A completed research (`rnd`) task and whether its result was adopted. */
export interface ResearchConclusion {
  task: Task;
  at: Timestamp;
  adopted: boolean;
}

/** The highlights of a year: merged pull request snapshots and research conclusions. */
export interface YearInReview {
  /** `null` when no completed task recorded a pull request snapshot. */
  pullRequests: PullRequestTotals | null;
  /** The merged pull requests with the most changed lines, largest first, at most three. */
  largest: { task: Task; pullRequest: PullRequestSnapshot }[];
  /** Completed research tasks, oldest first. */
  research: ResearchConclusion[];
}

const largestPullRequests = 3;

/**
 * Pull request snapshots and research conclusions of the tasks completed in `entries`;
 * `null` when there is neither.
 */
export function yearInReview(entries: ReviewEntry[]): YearInReview | null {
  const completed = entries.filter((entry) => entry.outcome === "completed");
  const merged = completed.flatMap(({ task }) => (task.pullRequest ? [{ task, pullRequest: task.pullRequest }] : []));
  const research = completed
    .filter(({ task }) => task.taskType === "rnd")
    .map(({ task, at }) => ({ task, at, adopted: task.resolution !== "not_adopted" }))
    .sort((a, b) => a.at - b.at || a.task.id - b.task.id);
  if (merged.length === 0 && research.length === 0) return null;
  const size = ({ pullRequest }: (typeof merged)[number]) => pullRequest.additions + pullRequest.deletions;
  return {
    pullRequests:
      merged.length === 0
        ? null
        : merged.reduce(
            (total, { pullRequest }) => ({
              pullRequests: total.pullRequests + 1,
              additions: total.additions + pullRequest.additions,
              deletions: total.deletions + pullRequest.deletions,
              changedFiles: total.changedFiles + pullRequest.changedFiles,
              reviews: total.reviews + pullRequest.reviewCount,
              reviewed: total.reviewed + (pullRequest.reviewCount > 0 ? 1 : 0),
            }),
            { pullRequests: 0, additions: 0, deletions: 0, changedFiles: 0, reviews: 0, reviewed: 0 },
          ),
    largest: [...merged]
      .sort((a, b) => size(b) - size(a) || a.task.id - b.task.id)
      .slice(0, largestPullRequests),
    research,
  };
}

/** Reviews of merged pull requests, e.g. `5 reviews on 3 of 4 pull requests`. */
export function formatReviews(totals: PullRequestTotals): string {
  return `${plural(totals.reviews, "review")} on ${totals.reviewed} of ${plural(totals.pullRequests, "pull request")}`;
}

function yearInReviewMarkdown(year: YearInReview): string[] {
  const lines = ["## Year in review", ""];
  if (year.pullRequests) {
    lines.push(
      `- Merged pull requests: ${formatChanges(year.pullRequests)}`,
      `- Reviews: ${formatReviews(year.pullRequests)}`,
    );
  }
  if (year.largest.length > 0) {
    lines.push("- Largest pull requests:");
    for (const { task, pullRequest } of year.largest) {
      lines.push(`  - [${task.taskType}] ${task.title} (${pullRequestStats(pullRequest)})`);
    }
  }
  if (year.research.length > 0) {
    lines.push("- Research conclusions:");
    for (const { task, adopted } of year.research) {
      const conclusion = task.completionNote ? `: ${oneLine(task.completionNote)}` : "";
      lines.push(`  - ${task.title} (${adopted ? "adopted" : "not adopted"})${conclusion}`);
    }
  }
  lines.push("");
  return lines;
}

/** The summed size of merged pull requests, e.g. `+120/-30 in 9 files (3 pull requests)`. */
export function formatChanges(changes: ChangeStat): string {
  return `+${changes.additions}/-${changes.deletions} in ${plural(changes.changedFiles, "file")} (${plural(
    changes.pullRequests,
    "pull request",
  )})`;
}

function periodMonths(period: ReviewPeriod): string[] {
  const [year, month] = parseDay(period.from);
  const last = period.to.slice(0, 7);
  const months: string[] = [];
  for (let index = 0; ; index += 1) {
    const current = dayOf(new Date(year, month + index, 1)).slice(0, 7);
    months.push(current);
    if (current >= last) return months;
  }
}

/** A duration in seconds as hours below a day, otherwise as days with one decimal. */
export function formatDuration(seconds: number): string {
  const hours = seconds / 3600;
  if (hours < 1) return "<1h";
  if (Math.round(hours) < 24) return `${Math.round(hours)}h`;
  return `${(hours / 24).toFixed(1)}d`;
}

function formatStat(stat: DurationStat | null): string {
  if (stat === null) return "no data";
  return `${formatDuration(stat.median)} (${stat.tasks} ${stat.tasks === 1 ? "task" : "tasks"})`;
}

export const durationKeys = ["leadTime", "cycleTime", "activeTime", "reviewTime"] as const;

export const durationLabels = {
  leadTime: "Median lead time (planned to completed)",
  cycleTime: "Median cycle time (started to completed)",
  activeTime: "Median active time (started to completed, without pauses)",
  reviewTime: "Median review time (in review to merged)",
} as const;

/** The review of `period` as Markdown, for pasting into a report. */
export function reviewMarkdown(period: ReviewPeriod, entries: ReviewEntry[], workspaces: Workspace[]): string {
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const lines = [`# ${periodTitles[period.kind]}: ${periodLabel(period)}`, ""];
  const groups = groupByOutcome(entries);
  if (groups.length === 0) {
    lines.push("No recorded activity.", "");
    return lines.join("\n");
  }
  const summary = summarize(period, entries, workspaces);
  lines.push(
    "## Summary",
    "",
    `- ${summary.outcomes.map(({ outcome, count }) => `${outcomeLabels[outcome]}: ${count}`).join(", ")}`,
    `- Commits: ${summary.commits}`,
  );
  if (summary.notAdopted > 0) {
    lines.push(`- Concluded without merging (not adopted): ${summary.notAdopted}`);
  }
  if (summary.completedByType.length > 0) {
    lines.push(
      `- Completed by type: ${summary.completedByType.map(({ taskType, count }) => `${taskType} ${count}`).join(", ")}`,
      `- Completed by workspace: ${summary.completedByWorkspace
        .map(({ workspace, count }) => `${workspace} ${count}`)
        .join(", ")}`,
    );
  }
  if (summary.changes) {
    lines.push(`- Changes in completed pull requests: ${formatChanges(summary.changes)}`);
  }
  for (const key of durationKeys) {
    lines.push(`- ${durationLabels[key]}: ${formatStat(summary[key])}`);
  }
  lines.push("");
  const year = period.kind === "year" ? yearInReview(entries) : null;
  if (year) lines.push(...yearInReviewMarkdown(year));
  if (summary.completedPerMonth) {
    lines.push("| Month | Completed |", "| --- | --- |");
    for (const { month, count } of summary.completedPerMonth) lines.push(`| ${month} | ${count} |`);
    lines.push("");
  }
  for (const group of groups) {
    lines.push(`## ${outcomeLabels[group.outcome]} (${group.entries.length})`, "");
    for (const { task, at, commits } of group.entries) {
      const notAdopted = task.resolution === "not_adopted";
      const details = [names.get(task.workspaceId) ?? `workspace ${task.workspaceId}`, formatDate(at)];
      if (notAdopted) details.push("not adopted");
      if (task.issue) details.push(task.issue.url);
      if (task.pullRequestUrl) details.push(task.pullRequestUrl);
      if (task.pullRequest) details.push(pullRequestStats(task.pullRequest));
      lines.push(`- [${task.taskType}] ${task.title} (${details.join(", ")})`);
      if (task.completionNote) {
        lines.push(`  - ${notAdopted ? "Conclusion" : "Note"}: ${oneLine(task.completionNote)}`);
      }
      for (const commit of commits) {
        lines.push(`  - \`${commit.sha.slice(0, 7)}\` ${commitSubject(commit.message)}`);
      }
    }
    lines.push("");
  }
  return lines.join("\n");
}

/** Local date and time of `at` as `YYYY-MM-DD HH:MM`, which spreadsheets read as a date. */
export function formatDateTime(at: Timestamp): string {
  const date = new Date(at * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${formatDate(at)} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function csvField(value: string | number | null): string {
  const text = value === null ? "" : String(value);
  return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

const csvColumns = [
  "Task",
  "Workspace",
  "Type",
  "Title",
  "Outcome",
  "Outcome at",
  "Resolution",
  "Planned",
  "Started",
  "In review",
  "Completed",
  "Merged",
  "Pull request",
  "Issue",
  "Commits in period",
  "Active hours",
  "Additions",
  "Deletions",
  "Changed files",
  "Reviews",
  "Completion note",
];

/**
 * One row per reviewed task as CSV (RFC 4180, CRLF line ends), led by a UTF-8 byte order
 * mark so spreadsheet apps read non-ASCII titles correctly. Active hours of a task still
 * being worked on count until `now`.
 */
export function reviewCsv(entries: ReviewEntry[], workspaces: Workspace[], now: Timestamp): string {
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const time = (at: Timestamp | null) => (at === null ? null : formatDateTime(at));
  const rows = entries.map(({ task, outcome, at, commits }) => [
    task.id,
    names.get(task.workspaceId) ?? `workspace ${task.workspaceId}`,
    task.taskType,
    task.title,
    outcomeLabels[outcome],
    formatDateTime(at),
    resolutionLabel(task),
    time(task.milestones.plannedAt),
    time(task.milestones.startedAt),
    time(task.milestones.reviewAt),
    time(task.milestones.completedAt),
    time(task.pullRequestMergedAt),
    task.pullRequestUrl,
    task.issue?.url ?? null,
    commits.length,
    activeHours(activeSeconds(task.timeline, now)),
    task.pullRequest?.additions ?? null,
    task.pullRequest?.deletions ?? null,
    task.pullRequest?.changedFiles ?? null,
    task.pullRequest?.reviewCount ?? null,
    task.completionNote && oneLine(task.completionNote),
  ]);
  return `\uFEFF${[csvColumns, ...rows].map((row) => row.map(csvField).join(",")).join("\r\n")}\r\n`;
}

function activeHours(seconds: number | null): number | null {
  return seconds === null ? null : Math.round(seconds / 360) / 10;
}

export function reviewFileName(period: ReviewPeriod, extension: "md" | "csv"): string {
  return `toodo-review-${periodLabel(period).replaceAll(" ", "-")}.${extension}`;
}
