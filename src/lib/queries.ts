import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  commands,
  type CommandError,
  type Machine,
  type MachineSettings,
  type NewSshHost,
  type NewTask,
  type NewWorkspace,
  type TaskEdit,
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
  commitRecords: (taskId: TaskId) => ["tasks", taskId, "commit-records"] as const,
  herdrMachines: ["herdr-machines"] as const,
  sshHosts: ["ssh-hosts"] as const,
  remoteDirectories: (machine: string, path: string | null) =>
    ["remote-directories", machine, path] as const,
};

/** Machines saved in herdr; refetched when the window regains focus. */
export function useHerdrMachines() {
  return useQuery({
    queryKey: queryKeys.herdrMachines,
    queryFn: () => unwrap(commands.listHerdrMachines()),
  });
}

/** Runs `herdr machine add <host>` in a terminal window; settles when that window closes. */
export function useAddHerdrMachine() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (host: string) => unwrap(commands.addHerdrMachine(host)),
    onSuccess: (machines) => queryClient.setQueryData(queryKeys.herdrMachines, machines),
  });
}

/** Host aliases of the user's SSH config; refetched when the window regains focus. */
export function useSshHosts() {
  return useQuery({
    queryKey: queryKeys.sshHosts,
    queryFn: () => unwrap(commands.listSshHosts()),
  });
}

export function useAddSshHost() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (host: NewSshHost) => unwrap(commands.addSshHost(host)),
    onSuccess: (hosts) => queryClient.setQueryData(queryKeys.sshHosts, hosts),
  });
}

/** Subdirectories of `path` on `machine`; `null` lists the login directory. */
export function useRemoteDirectories(machine: string, path: string | null) {
  return useQuery({
    queryKey: queryKeys.remoteDirectories(machine, path),
    queryFn: () => unwrap(commands.listRemoteDirectories(machine, path)),
    retry: false,
  });
}

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

export function useCommitRecords(taskId: TaskId, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.commitRecords(taskId),
    queryFn: () => unwrap(commands.listCommitRecords(taskId)),
    enabled,
  });
}

export function useRefreshCommitRecords() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (taskId: TaskId) => unwrap(commands.refreshCommitRecords(taskId)),
    onSuccess: (records, taskId) => queryClient.setQueryData(queryKeys.commitRecords(taskId), records),
  });
}

/** Pushes the task branch and opens (or reuses) its pull request; the task moves to `in_review`. */
export function useOpenPullRequest() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.openPullRequest(id)),
    onSuccess: (task) =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.commitRecords(task.id) }),
      ]),
  });
}

/** Marks an `in_review` task `done` once its pull request is merged. */
export function useCompleteTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.completeTask(id)),
    onSuccess: (task) => queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}

/** Removes the worktree of a `done` or `cancelled` task through herdr; `force` discards local changes. */
export function useRemoveTaskWorktree() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, force }: { id: TaskId; force: boolean }) =>
      unwrap(commands.removeTaskWorktree(id, force)),
    onSuccess: (task) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}

export function useUpdateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (task: TaskEdit) => unwrap(commands.updateTask(task)),
    onSuccess: (task) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}

/** A task of `workspaceId`; carried along so the workspace's task list can be refetched. */
type WorkspaceTask = { workspaceId: WorkspaceId; id: TaskId };

/** Commands that may change the status of several tasks of a workspace. */
function useWorkspaceTasksMutation<T extends { workspaceId: WorkspaceId }>(
  mutationFn: (variables: T) => Promise<unknown>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (_, { workspaceId }) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(workspaceId) }),
  });
}

export function useCancelTask() {
  return useWorkspaceTasksMutation(({ id }: WorkspaceTask) =>
    unwrap(commands.setTaskStatus(id, "cancelled")),
  );
}

export function useDeleteTask() {
  return useWorkspaceTasksMutation(({ id }: WorkspaceTask) => unwrap(commands.deleteTask(id)));
}

type Dependency = WorkspaceTask & { dependsOn: TaskId };

export function useAddDependency() {
  return useWorkspaceTasksMutation(({ id, dependsOn }: Dependency) =>
    unwrap(commands.addDependency(id, dependsOn)),
  );
}

export function useRemoveDependency() {
  return useWorkspaceTasksMutation(({ id, dependsOn }: Dependency) =>
    unwrap(commands.removeDependency(id, dependsOn)),
  );
}

export function useCreateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (task: NewTask) => unwrap(commands.createTask(task)),
    onSuccess: (task) =>
      queryClient.invalidateQueries({ queryKey: queryKeys.tasks(task.workspaceId) }),
  });
}
