import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  commands,
  type CommandError,
  type Machine,
  type MachineSettings,
  type NewTask,
  type NewWorkspace,
  type TaskId,
  type WorkspaceId,
  type WorkspaceLocation,
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
  herdrStatus: ["herdr-status"] as const,
  machineSettings: (machine: Machine) => ["machine-settings", machine] as const,
};

export function machineOf(location: WorkspaceLocation): Machine {
  return location.kind === "local" ? { kind: "local" } : { kind: "ssh", label: location.machine };
}

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

export function useHerdrStatus() {
  return useQuery({
    queryKey: queryKeys.herdrStatus,
    queryFn: () => unwrap(commands.herdrStatus()),
  });
}

export function useMachineSettings(machine: Machine) {
  return useQuery({
    queryKey: queryKeys.machineSettings(machine),
    queryFn: () => unwrap(commands.getMachineSettings(machine)),
  });
}

export function useSaveMachineSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (settings: MachineSettings) => unwrap(commands.saveMachineSettings(settings)),
    onSuccess: (settings) =>
      queryClient.setQueryData(queryKeys.machineSettings(settings.machine), settings),
  });
}

export function useStartTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.startTask(id)),
    onSuccess: (task) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}

export function useOpenTaskInHerdr() {
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.openTaskInHerdr(id)),
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
