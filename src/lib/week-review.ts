import type { ReviewEntry, ReviewOutcome, Timestamp, Workspace } from "@/lib/bindings";
import { formatDate } from "@/lib/timeline";

export const outcomeLabels: Record<ReviewOutcome, string> = {
  completed: "Completed",
  cancelled: "Cancelled",
  opened_for_review: "Opened for review",
  started: "Started",
  worked_on: "Worked on",
  planned: "Planned",
};

const outcomeOrder = Object.keys(outcomeLabels) as ReviewOutcome[];

function localMidnight(day: string, days = 0): Date {
  const [year, month, date] = day.split("-").map(Number);
  return new Date(year, month - 1, date + days);
}

/** The Monday of the week containing `date`, as `YYYY-MM-DD` in the local time zone. */
export function startOfWeek(date: Date): string {
  const sinceMonday = (date.getDay() + 6) % 7;
  return formatDate(new Date(date.getFullYear(), date.getMonth(), date.getDate() - sinceMonday).getTime() / 1000);
}

export function addDays(day: string, days: number): string {
  return formatDate(localMidnight(day, days).getTime() / 1000);
}

/**
 * The seven days from local midnight of `day` as `[start, end)` in seconds. Both ends are
 * local midnights, so a week with a daylight saving change is an hour shorter or longer.
 */
export function weekPeriod(day: string): { start: Timestamp; end: Timestamp } {
  return {
    start: localMidnight(day).getTime() / 1000,
    end: localMidnight(day, 7).getTime() / 1000,
  };
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

/** The review of the week starting on `weekStart` as Markdown, for pasting into a report. */
export function reviewMarkdown(weekStart: string, entries: ReviewEntry[], workspaces: Workspace[]): string {
  const names = new Map(workspaces.map((workspace) => [workspace.id, workspace.name]));
  const lines = [`# Week review: ${weekStart} to ${addDays(weekStart, 6)}`, ""];
  const groups = groupByOutcome(entries);
  if (groups.length === 0) {
    lines.push("No recorded activity.", "");
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
