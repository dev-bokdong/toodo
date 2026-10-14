import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReviewEntry, Task, Workspace } from "@/lib/bindings";
import { addDays, groupByOutcome, reviewMarkdown, startOfWeek, weekPeriod } from "@/lib/week-review";

afterEach(() => {
  vi.unstubAllEnvs();
});

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
      timeline: [],
      milestones: {
        plannedAt: null,
        readyAt: null,
        startedAt: null,
        reviewAt: null,
        completedAt: null,
        cancelledAt: null,
      },
      ...overrides,
    },
    outcome,
    at: new Date(2026, 9, 13, 15).getTime() / 1000,
    commits: [],
  };
}

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

describe("weekPeriod", () => {
  it("runs from local midnight to the local midnight seven days later", () => {
    vi.stubEnv("TZ", "Asia/Seoul");

    expect(weekPeriod("2026-10-12")).toEqual({
      start: Date.UTC(2026, 9, 11, 15) / 1000,
      end: Date.UTC(2026, 9, 18, 15) / 1000,
    });
  });

  it("keeps local midnights across a daylight saving change", () => {
    vi.stubEnv("TZ", "Europe/Berlin");

    const { start, end } = weekPeriod("2026-10-19");

    expect(start).toBe(Date.UTC(2026, 9, 18, 22) / 1000);
    expect(end).toBe(Date.UTC(2026, 9, 25, 23) / 1000);
    expect(end - start).toBe(7 * 24 * 3600 + 3600);
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

describe("reviewMarkdown", () => {
  const toodo: Workspace = {
    id: 1,
    name: "toodo",
    location: { kind: "local", path: "C:\\dev\\toodo" },
    defaultBaseBranch: "main",
  };

  it("lists each outcome with its tasks, pull requests, and commit subjects", () => {
    const done = entry(1, "completed", {
      title: "Add login",
      pullRequestUrl: "https://github.com/acme/toodo/pull/3",
    });
    done.commits = [
      {
        taskId: 1,
        sha: "0123456789abcdef",
        message: "Add login form\n\nWith validation.",
        author: "Ada <ada@example.com>",
        committedAt: done.at,
      },
    ];

    expect(reviewMarkdown("2026-10-12", [entry(2, "worked_on", { taskType: "fix" }), done], [toodo])).toBe(
      [
        "# Week review: 2026-10-12 to 2026-10-18",
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

  it("says so when the week has no recorded activity", () => {
    expect(reviewMarkdown("2026-10-12", [], [toodo])).toBe(
      "# Week review: 2026-10-12 to 2026-10-18\n\nNo recorded activity.\n",
    );
  });
});
