import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import type { AgentStatus, Task, TaskAgentStatus } from "@/lib/bindings";

export type AgentAlert = { title: string; body: string };

const alertTitles: Partial<Record<AgentStatus, string>> = {
  blocked: "Agent needs input",
  done: "Agent finished",
  idle: "Agent finished",
};

/**
 * Alerts for the agents whose status changed in a way the user should act on: one that
 * started waiting for input, or one that stopped working. A task that only now got an
 * agent, or one whose agent went away, raises no alert.
 */
export function agentAlerts(
  previous: readonly TaskAgentStatus[],
  next: readonly TaskAgentStatus[],
  tasks: readonly Task[],
): AgentAlert[] {
  const before = new Map(previous.map((entry) => [entry.taskId, entry.status]));
  return next.flatMap(({ taskId, status }) => {
    const was = before.get(taskId);
    if (was === undefined || was === status) {
      return [];
    }
    const finished = was === "working" && (status === "done" || status === "idle");
    if (status !== "blocked" && !finished) {
      return [];
    }
    const task = tasks.find((candidate) => candidate.id === taskId);
    const title = alertTitles[status] ?? "Agent status changed";
    return [{ title, body: task ? `#${taskId} ${task.title}` : `#${taskId}` }];
  });
}

/**
 * Alerts for tasks that were in progress and are now paused on a prerequisite that is not
 * done, with their herdr workspace still open: an agent added the prerequisite through MCP,
 * and the user decides in the app whether to close the workspace.
 */
export function pausedAlerts(previous: readonly Task[], next: readonly Task[]): AgentAlert[] {
  const before = new Map(previous.map((task) => [task.id, task.status]));
  const byId = new Map(next.map((task) => [task.id, task]));
  return next.flatMap((task) => {
    const blocked = task.dependsOn.some((id) => byId.get(id)?.status !== "done");
    const pausedNow = before.get(task.id) === "in_progress" && task.status === "planned";
    if (!pausedNow || task.herdrWorkspaceId === null || !blocked) {
      return [];
    }
    return [{ title: "Task paused by a new prerequisite", body: `#${task.id} ${task.title}: close its herdr workspace?` }];
  });
}

/** Shows the alerts as desktop notifications, asking for permission if needed. */
export async function notify(alerts: readonly AgentAlert[]): Promise<void> {
  if (alerts.length === 0) {
    return;
  }
  const granted = (await isPermissionGranted()) || (await requestPermission()) === "granted";
  if (!granted) {
    return;
  }
  for (const alert of alerts) {
    sendNotification(alert);
  }
}
