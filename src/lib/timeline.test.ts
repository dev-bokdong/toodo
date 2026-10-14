import { describe, expect, it } from "vitest";
import { formatDate, reachedMilestones } from "@/lib/timeline";

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
