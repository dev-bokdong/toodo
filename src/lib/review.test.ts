import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReviewEntry, Task, Workspace } from "@/lib/bindings";
import {
  addDays,
  formatDuration,
  groupByOutcome,
  periodBounds,
  periodContaining,
  periodLabel,
  reviewCsv,
  reviewFileName,
  reviewMarkdown,
  shiftPeriod,
  startOfWeek,
  summarize,
  weekStarting,
} from "@/lib/review";

afterEach(() => {
  vi.unstubAllEnvs();
});

const hour = 3600;
const day = 24 * hour;
const local = (year: number, month: number, date: number, hours = 0) =>
  new Date(year, month - 1, date, hours).getTime() / 1000;

function entry(id: number, outcome: ReviewEntry["outcome"], overrides: Partial<Task> = {}): ReviewEntry {
  return {
    task: {
      id,
      workspaceId: 1,
      taskType: "feat",
      title: `Task ${id}`,
      description: "",
      status: "in_progress",
      dependsOn: [],
      branch: null,
      worktreePath: null,
      herdrWorkspaceId: null,
      pullRequestUrl: null,
      pullRequestMergedAt: null,
      pullRequest: null,
      resolution: null,
      completionNote: null,
      timeline: [],
      milestones: {
        plannedAt: null,
        startedAt: null,
        reviewAt: null,
        completedAt: null,
        pausedAt: null,
      },
      ...overrides,
    },
    outcome,
    at: local(2026, 10, 13, 15),
    commits: [],
  };
}

function completed(id: number, at: number, milestones: Partial<Task["milestones"]>, overrides: Partial<Task> = {}) {
  const base = entry(id, "completed", { status: "done", ...overrides });
  base.task.milestones = { ...base.task.milestones, ...milestones, completedAt: at };
  base.at = at;
  return base;
}

function snapshot(number: number): NonNullable<Task["pullRequest"]> {
  return {
    number,
    title: `Pull request ${number}`,
    additions: 120,
    deletions: 30,
    changedFiles: 4,
    mergeCommitSha: "abc123",
    reviewCount: 2,
    reviewerCount: 1,
    firstReviewedAt: null,
  };
}

const toodo: Workspace = {
  id: 1,
  name: "toodo",
  location: { kind: "local", path: "C:\\dev\\toodo" },
  defaultBaseBranch: "main",
};
const herdr: Workspace = { ...toodo, id: 2, name: "herdr" };

describe("startOfWeek", () => {
  it("finds the Monday of the week, Sunday included", () => {
    expect(startOfWeek(new Date(2026, 9, 12, 0, 0))).toBe("2026-10-12");
    expect(startOfWeek(new Date(2026, 9, 14, 11, 30))).toBe("2026-10-12");
    expect(startOfWeek(new Date(2026, 9, 18, 23, 59))).toBe("2026-10-12");
    expect(startOfWeek(new Date(2026, 10, 1, 9))).toBe("2026-10-26");
  });
});

describe("addDays", () => {
  it("moves across month and year ends", () => {
    expect(addDays("2026-12-28", 7)).toBe("2027-01-04");
    expect(addDays("2026-03-02", -7)).toBe("2026-02-23");
  });
});

describe("periodContaining", () => {
  it("finds the week, month, quarter, and year of a day", () => {
    expect(periodContaining("week", "2026-10-14")).toEqual({ kind: "week", from: "2026-10-12", to: "2026-10-18" });
    expect(periodContaining("month", "2026-02-14")).toEqual({ kind: "month", from: "2026-02-01", to: "2026-02-28" });
    expect(periodContaining("quarter", "2026-12-31")).toEqual({
      kind: "quarter",
      from: "2026-10-01",
      to: "2026-12-31",
    });
    expect(periodContaining("quarter", "2026-01-01")).toEqual({
      kind: "quarter",
      from: "2026-01-01",
      to: "2026-03-31",
    });
    expect(periodContaining("year", "2026-10-14")).toEqual({ kind: "year", from: "2026-01-01", to: "2026-12-31" });
  });
});

describe("shiftPeriod", () => {
  it("moves whole periods across year ends", () => {
    expect(shiftPeriod(weekStarting("2026-12-28"), 1)).toEqual(weekStarting("2027-01-04"));
    expect(shiftPeriod(periodContaining("month", "2026-01-10"), -1)).toEqual({
      kind: "month",
      from: "2025-12-01",
      to: "2025-12-31",
    });
    expect(shiftPeriod(periodContaining("quarter", "2026-11-01"), 1)).toEqual({
      kind: "quarter",
      from: "2027-01-01",
      to: "2027-03-31",
    });
    expect(shiftPeriod(periodContaining("year", "2026-05-01"), -2)).toEqual({
      kind: "year",
      from: "2024-01-01",
      to: "2024-12-31",
    });
  });

  it("moves a custom period by its length in days", () => {
    expect(shiftPeriod({ kind: "custom", from: "2026-12-30", to: "2027-01-02" }, 1)).toEqual({
      kind: "custom",
      from: "2027-01-03",
      to: "2027-01-06",
    });
  });
});

describe("periodBounds", () => {
  it("runs from local midnight of the first day to local midnight after the last day", () => {
    vi.stubEnv("TZ", "Asia/Seoul");

    expect(periodBounds(weekStarting("2026-10-12"))).toEqual({
      start: Date.UTC(2026, 9, 11, 15) / 1000,
      end: Date.UTC(2026, 9, 18, 15) / 1000,
    });
    expect(periodBounds(periodContaining("year", "2026-06-01"))).toEqual({
      start: Date.UTC(2025, 11, 31, 15) / 1000,
      end: Date.UTC(2026, 11, 31, 15) / 1000,
    });
    expect(periodBounds({ kind: "custom", from: "2026-10-14", to: "2026-10-14" })).toEqual({
      start: Date.UTC(2026, 9, 13, 15) / 1000,
      end: Date.UTC(2026, 9, 14, 15) / 1000,
    });
  });

  it("keeps local midnights across a daylight saving change", () => {
    vi.stubEnv("TZ", "Europe/Berlin");

    const { start, end } = periodBounds(weekStarting("2026-10-19"));

    expect(start).toBe(Date.UTC(2026, 9, 18, 22) / 1000);
    expect(end).toBe(Date.UTC(2026, 9, 25, 23) / 1000);
    expect(end - start).toBe(7 * day + hour);
  });
});

describe("periodLabel and reviewFileName", () => {
  it("names each kind of period", () => {
    expect(periodLabel(weekStarting("2026-10-12"))).toBe("2026-10-12 to 2026-10-18");
    expect(periodLabel(periodContaining("month", "2026-10-12"))).toBe("2026-10");
    expect(periodLabel(periodContaining("quarter", "2026-10-12"))).toBe("2026 Q4");
    expect(periodLabel(periodContaining("year", "2026-10-12"))).toBe("2026");
    expect(reviewFileName(periodContaining("quarter", "2026-10-12"), "csv")).toBe("toodo-review-2026-Q4.csv");
  });
});

describe("groupByOutcome", () => {
  it("orders groups from most advanced and drops empty ones", () => {
    const groups = groupByOutcome([entry(1, "planned"), entry(2, "completed"), entry(3, "planned")]);

    expect(groups.map((group) => [group.outcome, group.entries.map((e) => e.task.id)])).toEqual([
      ["completed", [2]],
      ["planned", [1, 3]],
    ]);
  });
});

describe("summarize", () => {
  const year = periodContaining("year", "2026-06-01");

  it("counts outcomes, commits, and completed tasks by type and workspace", () => {
    const fix = completed(2, local(2026, 3, 5), {}, { taskType: "fix", workspaceId: 2 });
    const worked = entry(4, "worked_on");
    worked.commits = [
      { taskId: 4, sha: "a", message: "A", author: "Ada", committedAt: worked.at },
      { taskId: 4, sha: "b", message: "B", author: "Ada", committedAt: worked.at },
    ];
    const summary = summarize(
      year,
      [completed(1, local(2026, 3, 1), {}), fix, completed(3, local(2026, 10, 1), {}), worked, entry(5, "started")],
      [toodo, herdr],
    );

    expect(summary.outcomes).toEqual([
      { outcome: "completed", count: 3 },
      { outcome: "started", count: 1 },
      { outcome: "worked_on", count: 1 },
    ]);
    expect(summary.commits).toBe(2);
    expect(summary.completedByType).toEqual([
      { taskType: "feat", count: 2 },
      { taskType: "fix", count: 1 },
    ]);
    expect(summary.completedByWorkspace).toEqual([
      { workspace: "toodo", count: 2 },
      { workspace: "herdr", count: 1 },
    ]);
    expect(summary.completedPerMonth).toHaveLength(12);
    expect(summary.completedPerMonth?.[2]).toEqual({ month: "2026-03", count: 2 });
    expect(summary.completedPerMonth?.[9]).toEqual({ month: "2026-10", count: 1 });
    expect(summary.completedPerMonth?.[11]).toEqual({ month: "2026-12", count: 0 });
  });

  it("takes medians over completed tasks that reached both milestones", () => {
    const end = local(2026, 5, 20);
    const summary = summarize(
      year,
      [
        completed(1, end, { plannedAt: end - 10 * day, startedAt: end - 4 * day, reviewAt: end - day }),
        completed(2, end, { plannedAt: end - 2 * day, startedAt: end - 2 * day, reviewAt: end - 3 * hour }, {
          pullRequestMergedAt: end - hour,
        }),
        completed(3, end, { plannedAt: end - 6 * day }),
        entry(4, "started", {
          milestones: { ...entry(4, "started").task.milestones, plannedAt: end - 100 * day, startedAt: end },
        }),
      ],
      [toodo],
    );

    expect(summary.leadTime).toEqual({ median: 6 * day, tasks: 3 });
    expect(summary.cycleTime).toEqual({ median: 3 * day, tasks: 2 });
    expect(summary.activeTime).toBeNull();
    expect(summary.reviewTime).toEqual({ median: (day + 2 * hour) / 2, tasks: 2 });
  });

  it("leaves out durations without data and months for a period inside one month", () => {
    const summary = summarize(periodContaining("month", "2026-10-01"), [entry(1, "started")], [toodo]);

    expect(summary.leadTime).toBeNull();
    expect(summary.completedByType).toEqual([]);
    expect(summary.completedPerMonth).toBeNull();
    expect(summarize({ kind: "custom", from: "2026-10-30", to: "2026-11-02" }, [], []).completedPerMonth).toEqual([
      { month: "2026-10", count: 0 },
      { month: "2026-11", count: 0 },
    ]);
  });

  it("leaves pauses out of the active time of completed tasks", () => {
    const end = local(2026, 5, 20);
    const resumed = completed(1, end, { startedAt: end - 10 * day }, {
      timeline: [
        { status: "planned", at: end - 20 * day },
        { status: "in_progress", at: end - 10 * day },
        { status: "planned", at: end - 9 * day },
        { status: "in_progress", at: end - 2 * day },
        { status: "in_review", at: end - day },
        { status: "done", at: end },
      ],
    });

    const summary = summarize(year, [resumed], [toodo]);

    expect(summary.cycleTime).toEqual({ median: 10 * day, tasks: 1 });
    expect(summary.activeTime).toEqual({ median: 3 * day, tasks: 1 });
  });
});

describe("formatDuration", () => {
  it("shows hours below a day and days above", () => {
    expect(formatDuration(20 * 60)).toBe("<1h");
    expect(formatDuration(5 * hour + 40 * 60)).toBe("6h");
    expect(formatDuration(23.5 * hour)).toBe("1.0d");
    expect(formatDuration(36 * hour)).toBe("1.5d");
  });
});

describe("reviewMarkdown", () => {
  it("leads with a summary, then lists each outcome with its tasks, pull requests, and commit subjects", () => {
    const done = entry(1, "completed", {
      title: "Add login",
      pullRequestUrl: "https://github.com/acme/toodo/pull/3",
    });
    done.task.milestones = { ...done.task.milestones, plannedAt: done.at - 2 * day, completedAt: done.at };
    done.commits = [
      {
        taskId: 1,
        sha: "0123456789abcdef",
        message: "Add login form\n\nWith validation.",
        author: "Ada <ada@example.com>",
        committedAt: done.at,
      },
    ];

    expect(reviewMarkdown(weekStarting("2026-10-12"), [entry(2, "worked_on", { taskType: "fix" }), done], [toodo])).toBe(
      [
        "# Week review: 2026-10-12 to 2026-10-18",
        "",
        "## Summary",
        "",
        "- Completed: 1, Worked on: 1",
        "- Commits: 1",
        "- Completed by type: feat 1",
        "- Completed by workspace: toodo 1",
        "- Median lead time (planned to completed): 2.0d (1 task)",
        "- Median cycle time (started to completed): no data",
        "- Median active time (started to completed, without pauses): no data",
        "- Median review time (in review to merged): no data",
        "",
        "## Completed (1)",
        "",
        "- [feat] Add login (toodo, 2026-10-13, https://github.com/acme/toodo/pull/3)",
        "  - `0123456` Add login form",
        "",
        "## Worked on (1)",
        "",
        "- [fix] Task 2 (toodo, 2026-10-13)",
        "",
      ].join("\n"),
    );
  });

  it("adds a completed-per-month table for a period over several months", () => {
    const markdown = reviewMarkdown(
      periodContaining("quarter", "2026-10-01"),
      [completed(1, local(2026, 11, 3), {})],
      [toodo],
    );

    expect(markdown.startsWith("# Quarter review: 2026 Q4\n")).toBe(true);
    expect(markdown).toContain("| Month | Completed |\n| --- | --- |\n| 2026-10 | 0 |\n| 2026-11 | 1 |\n| 2026-12 | 0 |\n");
  });

  it("adds the size of merged pull requests and completion notes", () => {
    const at = local(2026, 10, 13, 15);
    const entries = [
      completed(1, at, {}, { pullRequest: snapshot(12), completionNote: "Sign-in is twice as fast\nfor everyone" }),
      completed(2, at, {}, { pullRequest: { ...snapshot(13), changedFiles: 1, reviewCount: 1 } }),
      entry(3, "started"),
    ];
    const markdown = reviewMarkdown(weekStarting("2026-10-12"), entries, [toodo]);

    expect(summarize(weekStarting("2026-10-12"), entries, [toodo]).withoutNote).toBe(1);

    expect(markdown).toContain("- Changes in completed pull requests: +240/-60 in 5 files (2 pull requests)\n");
    expect(markdown).toContain(
      "- [feat] Task 1 (toodo, 2026-10-13, #12, +120/-30 in 4 files, 2 reviews)\n  - Note: Sign-in is twice as fast for everyone\n",
    );
    expect(markdown).toContain("- [feat] Task 2 (toodo, 2026-10-13, #13, +120/-30 in 1 file, 1 review)\n");
  });

  it("counts work that was not adopted and lists its conclusion", () => {
    const at = local(2026, 10, 13, 15);
    const entries = [
      completed(1, at, {}, { resolution: "merged", completionNote: "Shipped" }),
      completed(2, at, {}, {
        taskType: "rnd",
        resolution: "not_adopted",
        pullRequestUrl: "https://github.com/acme/toodo/pull/5",
        completionNote: "Twice as slow\nas the current cache",
      }),
    ];
    const summary = summarize(weekStarting("2026-10-12"), entries, [toodo]);
    const markdown = reviewMarkdown(weekStarting("2026-10-12"), entries, [toodo]);

    expect(summary.notAdopted).toBe(1);
    expect(summary.withoutNote).toBe(0);
    expect(markdown).toContain("- Concluded without merging (not adopted): 1\n");
    expect(markdown).toContain("- [feat] Task 1 (toodo, 2026-10-13)\n  - Note: Shipped\n");
    expect(markdown).toContain(
      "- [rnd] Task 2 (toodo, 2026-10-13, not adopted, https://github.com/acme/toodo/pull/5)\n  - Conclusion: Twice as slow as the current cache\n",
    );
    expect(reviewMarkdown(weekStarting("2026-10-12"), entries.slice(0, 1), [toodo])).not.toContain(
      "not adopted",
    );
  });

  it("says so when the period has no recorded activity", () => {
    expect(reviewMarkdown(periodContaining("year", "2026-01-01"), [], [toodo])).toBe(
      "# Year review: 2026\n\nNo recorded activity.\n",
    );
  });
});

describe("reviewCsv", () => {
  it("writes one row per task with local times and quotes fields that need it", () => {
    const at = local(2026, 10, 13, 15);
    const done = completed(7, at, { plannedAt: local(2026, 10, 1, 9) }, {
      title: 'Fix "login", then\nlogout',
      pullRequestUrl: "https://github.com/acme/toodo/pull/3",
      pullRequestMergedAt: at - hour,
      pullRequest: snapshot(3),
      completionNote: "Cut login time\n  in half, finally",
      resolution: "not_adopted",
    });

    done.task.timeline = [
      { status: "planned", at: local(2026, 10, 1, 9) },
      { status: "in_progress", at: at - 3 * hour },
      { status: "in_review", at: at - 2 * hour },
      { status: "done", at },
    ];
    const working = entry(8, "planned", { workspaceId: 9, title: "Plan café" });
    working.task.timeline = [{ status: "in_progress", at: at - 1800 }];

    const csv = reviewCsv([done, working], [toodo], at);

    expect(csv).toBe(
      "\uFEFF" +
        [
          "Task,Workspace,Type,Title,Outcome,Outcome at,Resolution,Planned,Started,In review,Completed,Merged,Pull request,Commits in period,Active hours,Additions,Deletions,Changed files,Reviews,Completion note",
          '7,toodo,feat,"Fix ""login"", then\nlogout",Completed,2026-10-13 15:00,Not adopted,2026-10-01 09:00,,,2026-10-13 15:00,2026-10-13 14:00,https://github.com/acme/toodo/pull/3,0,3,120,30,4,2,"Cut login time in half, finally"',
          "8,workspace 9,feat,Plan café,Planned,2026-10-13 15:00,,,,,,,,0,0.5,,,,,",
        ].join("\r\n") +
        "\r\n",
    );
  });
});
