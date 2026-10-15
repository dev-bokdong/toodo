import { describe, expect, it } from "vitest";
import { agentAlerts } from "@/lib/agent-alerts";
import type { Task } from "@/lib/bindings";

const tasks = [{ id: 1, title: "Add login" }, { id: 2, title: "Add logout" }] as Task[];

describe("agentAlerts", () => {
  it("alerts when an agent starts waiting for input", () => {
    expect(agentAlerts([{ taskId: 1, status: "working" }], [{ taskId: 1, status: "blocked" }], tasks)).toEqual([
      { title: "Agent needs input", body: "#1 Add login" },
    ]);
  });

  it("alerts when a working agent finishes", () => {
    expect(
      agentAlerts(
        [
          { taskId: 1, status: "working" },
          { taskId: 2, status: "working" },
        ],
        [
          { taskId: 1, status: "done" },
          { taskId: 2, status: "idle" },
        ],
        tasks,
      ),
    ).toEqual([
      { title: "Agent finished", body: "#1 Add login" },
      { title: "Agent finished", body: "#2 Add logout" },
    ]);
  });

  it("stays quiet for unchanged, new, resumed, or idle-to-done agents", () => {
    expect(
      agentAlerts(
        [
          { taskId: 1, status: "blocked" },
          { taskId: 2, status: "idle" },
        ],
        [
          { taskId: 1, status: "working" },
          { taskId: 2, status: "done" },
          { taskId: 3, status: "blocked" },
        ],
        tasks,
      ),
    ).toEqual([]);
    expect(agentAlerts([{ taskId: 1, status: "blocked" }], [{ taskId: 1, status: "blocked" }], tasks)).toEqual([]);
  });

  it("falls back to the task ID when the task is not loaded", () => {
    expect(agentAlerts([{ taskId: 9, status: "working" }], [{ taskId: 9, status: "blocked" }], tasks)).toEqual([
      { title: "Agent needs input", body: "#9" },
    ]);
  });
});
