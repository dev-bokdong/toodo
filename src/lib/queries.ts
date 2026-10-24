import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  commands,
  events,
  type BackupSettings,
  type CommandError,
  type Machine,
  type MachineSettings,
  type MemoryContent,
  type MemoryId,
  type NewSshHost,
  type NewTask,
  type NewWorkspace,
  type Task,
  type TaskEdit,
  type TaskId,
  type TaskAgentStatus,
  type TaskPullRequestStatus,
  type Timestamp,
  type Workspace,
  type WorkspaceId,
  type WorkspaceLocation,
} from "@/lib/bindings";
import { downloadBackup, downloadExport, uploadRestore } from "@/web/files";
import { agentAlerts, notify, pullRequestAlerts, taskStatusAlerts, type AgentAlert } from "@/lib/agent-alerts";

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
  /** Tasks of every workspace: dependencies cross workspaces, so they are loaded together. */
  tasks: ["task-list"] as const,
  herdrStatus: ["herdr-status"] as const,
  agentStatuses: ["agent-statuses"] as const,
  pullRequestStatuses: ["pull-request-statuses"] as const,
  machineSettings: (machine: Machine) => ["machine-settings", machine] as const,
  branches: (workspace: WorkspaceId) => ["branches", workspace] as const,
  commitRecords: (taskId: TaskId) => ["tasks", taskId, "commit-records"] as const,
  branchFreshness: (taskId: TaskId) => ["tasks", taskId, "branch-freshness"] as const,
  herdrMachines: ["herdr-machines"] as const,
  sshHosts: ["ssh-hosts"] as const,
  backups: ["backups"] as const,
  backupSettings: ["backup-settings"] as const,
  mcpServerInfo: ["mcp-server-info"] as const,
  mcpTunnels: ["mcp-tunnels"] as const,
  review: (start: Timestamp, end: Timestamp) => ["review", start, end] as const,
  directories: (machine: Machine, path: string | null) => ["directories", machine, path] as const,
  /** Memory entries of a workspace, or the shared ones for `null`. */
  memory: (workspace: WorkspaceId | null, includeSuperseded: boolean) =>
    ["memory", workspace, includeSuperseded] as const,
  allMemory: ["memory"] as const,
};

export function useMemoryEntries(workspace: WorkspaceId | null, includeSuperseded: boolean) {
  return useQuery({
    queryKey: queryKeys.memory(workspace, includeSuperseded),
    queryFn: () => unwrap(commands.listMemory(workspace, includeSuperseded)),
  });
}

export function useAddMemory() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ workspace, content }: { workspace: WorkspaceId | null; content: MemoryContent }) =>
      unwrap(commands.addMemory(workspace, content)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.allMemory }),
  });
}

export function useUpdateMemory() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, content }: { id: MemoryId; content: MemoryContent }) =>
      unwrap(commands.updateMemory(id, content)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.allMemory }),
  });
}

export function useSupersedeMemory() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, content }: { id: MemoryId; content: MemoryContent }) =>
      unwrap(commands.supersedeMemory(id, content)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.allMemory }),
  });
}

/** Where agents reach Toodo's MCP server; fixed while the app runs. */
export function useMcpServerInfo() {
  return useQuery({
    queryKey: queryKeys.mcpServerInfo,
    queryFn: () => commands.mcpServerInfo(),
    staleTime: Infinity,
  });
}

/** Reverse tunnels that let agents on SSH machines reach the MCP server. */
export function useMcpTunnels() {
  return useQuery({
    queryKey: queryKeys.mcpTunnels,
    queryFn: () => commands.listMcpTunnels(),
    refetchInterval: 3000,
  });
}

/** Backups in Toodo's backup directory, newest first. */
export function useBackups() {
  return useQuery({
    queryKey: queryKeys.backups,
    queryFn: () => unwrap(commands.listBackups()),
  });
}

export function useBackupSettings() {
  return useQuery({
    queryKey: queryKeys.backupSettings,
    queryFn: () => commands.getBackupSettings(),
  });
}

/** Saving may write an automatic backup right away, so the backup list is refetched. */
export function useSaveBackupSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (settings: BackupSettings) => unwrap(commands.saveBackupSettings(settings)),
    onSuccess: (settings) => {
      queryClient.setQueryData(queryKeys.backupSettings, settings);
      return queryClient.invalidateQueries({ queryKey: queryKeys.backups });
    },
  });
}

export function useExportDatabase() {
  return useMutation({
    mutationFn: (path: string) => unwrap(commands.exportDatabase(path)),
  });
}

/** Replaces all data with a backup, so every query is refetched afterwards. */
export function useRestoreDatabase() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (path: string) => unwrap(commands.restoreDatabase(path)),
    onSuccess: () => queryClient.invalidateQueries(),
  });
}

/** Replaces all data with a backup from Toodo's backup directory, by its file name. */
export function useRestoreBackup() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => unwrap(commands.restoreBackup(name)),
    onSuccess: () => queryClient.invalidateQueries(),
  });
}

/** Web build: downloads a snapshot of the server's database; resolves with its file name. */
export function useDownloadExport() {
  return useMutation({ mutationFn: downloadExport });
}

/** Web build: downloads a backup from the server's backup directory. */
export function useDownloadBackup() {
  return useMutation({ mutationFn: downloadBackup });
}

/** Web build: replaces all data on the server with an uploaded database file. */
export function useUploadRestore() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: uploadRestore,
    onSuccess: () => queryClient.invalidateQueries(),
  });
}

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

/**
 * Subdirectories of `path` on `machine` (the server for `local` in the web build); `null`
 * lists the home directory.
 */
export function useDirectories(machine: Machine, path: string | null) {
  return useQuery({
    queryKey: queryKeys.directories(machine, path),
    queryFn: () => unwrap(commands.listDirectories(machine, path)),
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

/** Saves an edited workspace, such as a new default base branch. */
export function useUpdateWorkspace() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (workspace: Workspace) => unwrap(commands.updateWorkspace(workspace)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.workspaces, exact: true }),
  });
}

/**
 * Branches of the workspace repository, fetched from `origin` each time the base branch
 * picker opens so the list matches the remote.
 */
export function useWorkspaceBranches(workspace: WorkspaceId, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.branches(workspace),
    queryFn: () => unwrap(commands.listWorkspaceBranches(workspace)),
    enabled,
    staleTime: 0,
  });
}

/**
 * How far a started task's branch is behind its base and whether its pull request conflicts.
 * Each check fetches the base from origin, so it runs every few minutes rather than on every render.
 */
export function useBranchFreshness(taskId: TaskId, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.branchFreshness(taskId),
    queryFn: () => unwrap(commands.branchFreshness(taskId)),
    enabled,
    staleTime: 60_000,
    refetchInterval: 5 * 60_000,
    retry: false,
  });
}

/** Creates a branch from an existing one, pushes it, and makes it the workspace base. */
export function useCreateBaseBranch() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ workspace, name, from }: { workspace: WorkspaceId; name: string; from: string }) =>
      unwrap(commands.createWorkspaceBaseBranch(workspace, name, from)),
    onSuccess: (workspace) =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.workspaces, exact: true }),
        queryClient.invalidateQueries({ queryKey: queryKeys.branches(workspace.id) }),
      ]),
  });
}

/** Removes a workspace from Toodo with its tasks; the repository and herdr stay untouched. */
export function useDeleteWorkspace() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: WorkspaceId) => unwrap(commands.deleteWorkspace(id)),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
        queryClient.invalidateQueries({ queryKey: queryKeys.workspaces, exact: true }),
      ]),
  });
}

/** Tasks of every workspace; a status change may release dependents in other workspaces. */
export function useTasks() {
  return useQuery({
    queryKey: queryKeys.tasks,
    queryFn: () => unwrap(commands.listTasks()),
  });
}

/** Tasks of every workspace with a milestone or commit in `[start, end)`. */
export function useReview(start: Timestamp, end: Timestamp) {
  return useQuery({
    queryKey: queryKeys.review(start, end),
    queryFn: () => unwrap(commands.reviewPeriod(start, end)),
  });
}

/** Writes a Markdown or CSV report to a path picked in the save dialog. */
export function useSaveReport() {
  return useMutation({
    mutationFn: ({ path, contents }: { path: string; contents: string }) =>
      unwrap(commands.saveReport(path, contents)),
  });
}

export function useHerdrStatus() {
  return useQuery({
    queryKey: queryKeys.herdrStatus,
    queryFn: () => unwrap(commands.herdrStatus()),
  });
}

/** Agent status of each task whose herdr workspace runs an agent; kept current by `useHerdrEvents`. */
export function useAgentStatuses() {
  return useQuery({
    queryKey: queryKeys.agentStatuses,
    queryFn: () => commands.listAgentStatuses(),
  });
}

/** CI and review state of the pull request of each `in_review` task; kept current by `useHerdrEvents`. */
export function usePullRequestStatuses() {
  return useQuery({
    queryKey: queryKeys.pullRequestStatuses,
    queryFn: () => commands.listPullRequestStatuses(),
  });
}

/** Applies the changes the backend reports from herdr events and pull request checks while the app is mounted. */
export function useHerdrEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    /**
     * Stores `next` under `key` and notifies the alerts `alerts` finds against the previous
     * snapshot. Without a previous snapshot nothing is known to have changed. The app being
     * in focus means the user already sees the change on the task.
     */
    function update<T>(
      key: readonly string[],
      next: T[],
      alerts: (previous: T[], next: T[], tasks: Task[]) => AgentAlert[],
    ) {
      const previous = queryClient.getQueryData<T[]>(key);
      queryClient.setQueryData(key, next);
      if (previous === undefined || document.hasFocus()) {
        return;
      }
      const tasks = queryClient.getQueryData<Task[]>(queryKeys.tasks) ?? [];
      notify(alerts(previous, next, tasks)).catch((error: unknown) =>
        console.error("toodo: failed to show notifications", error),
      );
    }

    const subscriptions = [
      events.tasksChanged.listen(async () => {
        const previous = queryClient.getQueryData<Task[]>(queryKeys.tasks);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
          // A task completed in the background got the commits of its pull request.
          queryClient.invalidateQueries({ queryKey: ["tasks"] }),
        ]);
        if (previous === undefined || document.hasFocus()) {
          return;
        }
        const next = queryClient.getQueryData<Task[]>(queryKeys.tasks) ?? [];
        notify(taskStatusAlerts(previous, next)).catch((error: unknown) =>
          console.error("toodo: failed to show status notifications", error),
        );
      }),
      events.workspacesChanged.listen(() =>
        queryClient.invalidateQueries({ queryKey: queryKeys.workspaces, exact: true }),
      ),
      events.memoryChanged.listen(() => queryClient.invalidateQueries({ queryKey: queryKeys.allMemory })),
      events.agentStatusesChanged.listen(({ payload }) =>
        update<TaskAgentStatus>(queryKeys.agentStatuses, payload.statuses, agentAlerts),
      ),
      events.pullRequestStatusesChanged.listen(({ payload }) =>
        update<TaskPullRequestStatus>(queryKeys.pullRequestStatuses, payload.statuses, pullRequestAlerts),
      ),
    ];
    return () => {
      for (const subscription of subscriptions) {
        void subscription.then((unlisten) => unlisten());
      }
    };
  }, [queryClient]);
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
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
  });
}

export function useOpenTaskInHerdr() {
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.openTaskInHerdr(id)),
  });
}

/** Focuses the pane of the agent working on a task, or its herdr workspace when no agent is recognized. */
export function useFocusAgentPane() {
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.focusAgentPane(id)),
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
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
        queryClient.invalidateQueries({ queryKey: queryKeys.commitRecords(task.id) }),
      ]),
  });
}

/** Marks an `in_review` task `done` once its pull request is merged. */
export function useCompleteTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.completeTask(id)),
    // Completing records the commits of the merged pull request.
    onSuccess: (task) =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
        queryClient.invalidateQueries({ queryKey: queryKeys.commitRecords(task.id) }),
      ]),
  });
}

/**
 * Concludes an active task without merging it, e.g. research that is not adopted: it becomes
 * `done` as `not_adopted` with `note` as its conclusion, and its open pull request is closed.
 */
export function useConcludeTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, note }: { id: TaskId; note: string }) => unwrap(commands.concludeTask(id, note)),
    // Concluding records the commits of the branch.
    onSuccess: (task) =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
        queryClient.invalidateQueries({ queryKey: queryKeys.commitRecords(task.id) }),
        queryClient.invalidateQueries({ queryKey: ["review"] }),
      ]),
  });
}

/** Saves the completion note of a done task; an empty note clears it. */
export function useSetCompletionNote() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, note }: { id: TaskId; note: string }) => unwrap(commands.setCompletionNote(id, note)),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
        queryClient.invalidateQueries({ queryKey: ["review"] }),
      ]),
  });
}

/** Checks out the branch of an active task that lost its worktree into a new herdr worktree. */
export function useRestoreTaskWorktree() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: TaskId) => unwrap(commands.restoreTaskWorktree(id)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
  });
}

export function useUpdateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (task: TaskEdit) => unwrap(commands.updateTask(task)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
  });
}

/** Commands that change tasks that other tasks, possibly in other workspaces, may wait on. */
function useTasksMutation<T>(mutationFn: (variables: T) => Promise<unknown>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
  });
}

/** Pauses a started task: it becomes planned again and keeps its branch and worktree. */
export function usePauseTask() {
  return useTasksMutation((id: TaskId) => unwrap(commands.pauseTask(id)));
}

/** Closes the herdr workspace of a paused task; its worktree is kept for resuming. */
export function useCloseTaskWorkspace() {
  return useTasksMutation((id: TaskId) => unwrap(commands.closeTaskWorkspace(id)));
}

/** Deletes a task, first removing its worktree through herdr; `force` discards local changes. */
export function useDeleteTask() {
  return useTasksMutation(({ id, force }: { id: TaskId; force: boolean }) =>
    unwrap(commands.deleteTask(id, force)),
  );
}

/** The edge `id -> dependsOn`; `dependsOn` may be a task of another workspace. */
type Dependency = { id: TaskId; dependsOn: TaskId };

export function useAddDependency() {
  return useTasksMutation(({ id, dependsOn }: Dependency) => unwrap(commands.addDependency(id, dependsOn)));
}

export function useRemoveDependency() {
  return useTasksMutation(({ id, dependsOn }: Dependency) => unwrap(commands.removeDependency(id, dependsOn)));
}

/** Links GitHub issue `number` of the workspace repository to a task; `null` unlinks it. */
export function useLinkIssue() {
  return useTasksMutation(({ id, number }: { id: TaskId; number: number | null }) =>
    unwrap(commands.linkIssue(id, number)),
  );
}

/** Opens a GitHub issue from the title and description of a task and links it. */
export function useCreateIssue() {
  return useTasksMutation((id: TaskId) => unwrap(commands.createIssue(id)));
}

/** The task was created, but its issue could not be linked. */
export class IssueLinkFailure extends Error {
  constructor(taskId: TaskId, issue: number, reason: string) {
    super(`Task #${taskId} was added, but issue #${issue} could not be linked: ${reason}`);
    this.name = "IssueLinkFailure";
  }
}

/** Creates a task, then links issue `issue` to it when one is given. */
export function useCreateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({ issue, ...task }: NewTask & { issue: number | null }) => {
      const created = await unwrap(commands.createTask(task));
      if (issue === null) {
        return created;
      }
      try {
        return await unwrap(commands.linkIssue(created.id, issue));
      } catch (error) {
        throw new IssueLinkFailure(created.id, issue, error instanceof Error ? error.message : String(error));
      }
    },
    // A task whose issue could not be linked was still created.
    onSettled: () => queryClient.invalidateQueries({ queryKey: queryKeys.tasks }),
  });
}
