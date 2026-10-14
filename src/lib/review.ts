import type { ReviewEntry, ReviewOutcome, TaskType, Timestamp, Workspace } from "@/lib/bindings";
import { formatDate } from "@/lib/timeline";

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
  /** Opened for review to merged, or to completed when the merge time is unknown. */
  reviewTime: DurationStat | null;
  /** Tasks completed in each calendar month of the period; `null` for a period within one month. */
  completedPerMonth: { month: string; count: number }[] | null;
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
  };
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

export const durationLabels = {
  leadTime: "Median lead time (planned to completed)",
  cycleTime: "Median cycle time (started to completed)",
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
  if (summary.completedByType.length > 0) {
    lines.push(
      `- Completed by type: ${summary.completedByType.map(({ taskType, count }) => `${taskType} ${count}`).join(", ")}`,
      `- Completed by workspace: ${summary.completedByWorkspace
        .map(({ workspace, count }) => `${workspace} ${count}`)
        .join(", ")}`,
    );
  }
  for (const key of ["leadTime", "cycleTime", "reviewTime"] as const) {
    lines.push(`- ${durationLabels[key]}: ${formatStat(summary[key])}`);
  }
  lines.push("");
  if (summary.completedPerMonth) {
    lines.push("| Month | Completed |", "| --- | --- |");
    for (const { month, count } of summary.completedPerMonth) lines.push(`| ${month} | ${count} |`);
    lines.push("");
  }
  for (const group of groups) {
    lines.push(`## ${outcomeLabels[group.outcome]} (${group.entries.length})`, "");
    for (const { task, at, commits } of group.entries) {
      const details = [names.get(task.workspaceId) ?? `workspace ${task.workspaceId}`, formatDate(at)];
      if (task.pullRequestUrl) details.push(task.pullRequestUrl);
      lines.push(`- [${task.taskType}] ${task.title} (${details.join(", ")})`);
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
  "Planned",
  "Started",
  "In review",
  "Completed",
  "Merged",
  "Pull request",
  "Commits in period",
];

/**
 * One row per reviewed task as CSV (RFC 4180, CRLF line ends), led by a UTF-8 byte order
 * mark so spreadsheet apps read non-ASCII titles correctly.
 */
export function reviewCsv(entries: ReviewEntry[], workspaces: Workspace[]): string {
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const time = (at: Timestamp | null) => (at === null ? null : formatDateTime(at));
  const rows = entries.map(({ task, outcome, at, commits }) => [
    task.id,
    names.get(task.workspaceId) ?? `workspace ${task.workspaceId}`,
    task.taskType,
    task.title,
    outcomeLabels[outcome],
    formatDateTime(at),
    time(task.milestones.plannedAt),
    time(task.milestones.startedAt),
    time(task.milestones.reviewAt),
    time(task.milestones.completedAt),
    time(task.pullRequestMergedAt),
    task.pullRequestUrl,
    commits.length,
  ]);
  return `\uFEFF${[csvColumns, ...rows].map((row) => row.map(csvField).join(",")).join("\r\n")}\r\n`;
}

export function reviewFileName(period: ReviewPeriod, extension: "md" | "csv"): string {
  return `toodo-review-${periodLabel(period).replaceAll(" ", "-")}.${extension}`;
}
