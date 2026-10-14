import { describe, expect, it } from "vitest";
import type { TaskEvent, TaskStatus } from "@/lib/bindings";
import { activeSeconds, formatDate, reachedMilestones, workSegments } from "@/lib/timeline";

function history(raw: [TaskStatus, number][]): TaskEvent[] {
  return raw.map(([status, at]) => ({ status, at }));
}

const resumed = history([
  ["planned", 0],
  ["in_progress", 100],
  ["planned", 300],
  ["in_progress", 1000],
  ["in_review", 1100],
  ["done", 1200],
]);

describe("workSegments", () => {
  it("splits the work from the first start to completion into stretches, pauses included", () => {
    expect(workSegments(resumed)).toEqual([
      { state: "in_progress", start: 100, end: 300 },
      { state: "paused", start: 300, end: 1000 },
      { state: "in_progress", start: 1000, end: 1100 },
      { state: "in_review", start: 1100, end: 1200 },
    ]);
  });

  it("leaves the current stretch open and ignores the time before the first start", () => {
    expect(workSegments(history([["planned", 0]]))).toEqual([]);
    expect(workSegments(history([["planned", 0], ["in_progress", 50], ["planned", 80]]))).toEqual([
      { state: "in_progress", start: 50, end: 80 },
      { state: "paused", start: 80, end: null },
    ]);
  });
});

describe("activeSeconds", () => {
  it("counts time in progress and in review but not pauses", () => {
    expect(activeSeconds(resumed, 5000)).toBe(400);
  });

  it("counts a stretch under way until now, and is null without a start", () => {
    expect(activeSeconds(history([["planned", 0], ["in_progress", 50]]), 90)).toBe(40);
    expect(activeSeconds(history([["planned", 0], ["in_progress", 50], ["planned", 80]]), 900)).toBe(30);
    expect(activeSeconds([], 90)).toBeNull();
  });
});

describe("formatDate", () => {
  it("prints the local calendar date", () => {
    const at = new Date(2026, 0, 5, 23, 30).getTime() / 1000;

    expect(formatDate(at)).toBe("2026-01-05");
  });
});

describe("reachedMilestones", () => {
  it("lists reached milestones in lifecycle order and skips the rest", () => {
    expect(
      reachedMilestones({
        plannedAt: 10,
        startedAt: 20,
        reviewAt: null,
        completedAt: null,
        pausedAt: 30,
      }),
    ).toEqual([
      { label: "Planned", at: 10 },
      { label: "Started", at: 20 },
      { label: "Paused", at: 30 },
    ]);
  });
});
