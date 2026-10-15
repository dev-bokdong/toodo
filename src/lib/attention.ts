import type { AgentStatus, Task, TaskAgentStatus, TaskPullRequestStatus } from "@/lib/bindings";

/** Why a task waits on the user, most urgent first. */
export const attentionReasons = ["agent_blocked", "blocker_note", "ci_failed", "changes_requested"] as const;

export type AttentionReason = (typeof attentionReasons)[number];

export type AttentionItem = {
  task: Task;
  reasons: AttentionReason[];
  agentStatus: AgentStatus | null;
};

/**
 * Tasks that wait on the user: an agent asking for input, a blocker an agent reported as its
 * latest note, or a pull request whose CI failed or that got changes requested. The most
 * urgent come first, then by task ID.
 */
export function needsAttention(
  tasks: Task[],
  agentStatuses: TaskAgentStatus[],
  pullRequestStatuses: TaskPullRequestStatus[],
): AttentionItem[] {
  const agents = new Map(agentStatuses.map((entry) => [entry.taskId, entry.status]));
  const pullRequests = new Map(pullRequestStatuses.map((entry) => [entry.taskId, entry]));
  const rank = (item: AttentionItem) => attentionReasons.indexOf(item.reasons[0]);
  return tasks
    .flatMap((task) => {
      const active = task.status === "in_progress" || task.status === "in_review";
      const agentStatus = active ? (agents.get(task.id) ?? null) : null;
      // Statuses are only checked while the pull request is open.
      const pullRequest = task.status === "in_review" ? pullRequests.get(task.id) : undefined;
      const reasons = attentionReasons.filter((reason) => {
        switch (reason) {
          case "agent_blocked":
            return agentStatus === "blocked";
          case "blocker_note":
            // A later progress note means the agent got past the blocker.
            return task.status !== "done" && task.notes.at(-1)?.kind === "blocker";
          case "ci_failed":
            return pullRequest?.checks === "failing";
          case "changes_requested":
            return pullRequest?.review === "changes_requested";
        }
      });
      return reasons.length > 0 ? [{ task, reasons, agentStatus }] : [];
    })
    .sort((a, b) => rank(a) - rank(b) || a.task.id - b.task.id);
}
