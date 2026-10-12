import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  commands,
  type CommandError,
  type NewTask,
  type NewWorkspace,
  type WorkspaceId,
} from "@/lib/bindings";

type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

export class CommandFailure extends Error {
  readonly kind: CommandError["kind"];

  constructor(error: CommandError) {
    super(error.message);
    this.name = "CommandFailure";
    this.kind = error.kind;
  }
}

async function unwrap<T>(result: Promise<CommandResult<T>>): Promise<T> {
  const outcome = await result;
  if (outcome.status === "error") {
    throw new CommandFailure(outcome.error);
  }
  return outcome.data;
}

export const queryKeys = {
  workspaces: ["workspaces"] as const,
  tasks: (workspaceId: WorkspaceId) => ["workspaces", workspaceId, "tasks"] as const,
};

export function useWorkspaces() {
  return useQuery({
    queryKey: queryKeys.workspaces,
    queryFn: () => unwrap(commands.listWorkspaces()),
  });
}

export function useCreateWorkspace() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (workspace: NewWorkspace) => unwrap(commands.createWorkspace(workspace)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.workspaces }),
  });
}

export function useTasks(workspaceId: WorkspaceId) {
  return useQuery({
    queryKey: queryKeys.tasks(workspaceId),
    queryFn: () => unwrap(commands.listTasks(workspaceId)),
  });
}

export function useCreateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (task: NewTask) => unwrap(commands.createTask(task)),
    onSuccess: (task) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}
