import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { TaskGraph } from "@/components/task-graph";
import { inputClass, describeLocation } from "@/components/workspace-list";
import type { AgentStatus, Task, TaskStatus, TaskType, Workspace } from "@/lib/bindings";
import {
  CommandFailure,
  machineOf,
  useAddDependency,
  useAgentStatuses,
  useCancelTask,
  useCommitRecords,
  useCompleteTask,
  useCreateTask,
  useDeleteTask,
  useHerdrStatus,
  useMachineSettings,
  useOpenPullRequest,
  useOpenTaskInHerdr,
  useRefreshCommitRecords,
  useRemoveDependency,
  useRemoveTaskWorktree,
  useSaveMachineSettings,
  useStartTask,
  useTasks,
  useUpdateTask,
} from "@/lib/queries";
import { formatDate, reachedMilestones } from "@/lib/timeline";
import { cn } from "@/lib/utils";

const taskTypes: TaskType[] = ["feat", "fix", "modify", "refactor", "docs", "test", "chore"];

const agentLabels: Record<AgentStatus, string> = {
  idle: "Agent idle",
  working: "Agent working",
  blocked: "Agent needs input",
  done: "Agent done",
};

const agentStyles: Record<AgentStatus, string> = {
  idle: "bg-muted text-muted-foreground",
  working: "bg-sky-100 text-sky-800",
  blocked: "bg-amber-100 text-amber-900",
  done: "bg-emerald-100 text-emerald-800",
};

const statusLabels: Record<TaskStatus, string> = {
  planned: "Planned",
  ready: "Ready",
  in_progress: "In progress",
  in_review: "In review",
  done: "Done",
  cancelled: "Cancelled",
};

const statusStyles: Record<TaskStatus, string> = {
  planned: "bg-muted text-muted-foreground",
  ready: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200",
  in_progress: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-200",
  in_review: "bg-violet-100 text-violet-800 dark:bg-violet-950 dark:text-violet-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
  cancelled: "bg-muted text-muted-foreground line-through",
};

export function TaskList({ workspace }: { workspace: Workspace }) {
  const tasks = useTasks(workspace.id);
  const [view, setView] = useState<"list" | "timeline">("list");

  return (
    <section aria-labelledby="tasks-heading" className="flex flex-col gap-4">
      <header>
        <h2 id="tasks-heading" className="text-xl font-semibold tracking-tight">
          {workspace.name}
        </h2>
        <p className="text-sm text-muted-foreground">
          {describeLocation(workspace.location)} &middot; base {workspace.defaultBaseBranch}
        </p>
      </header>

      <WorktreeRootForm workspace={workspace} />

      <NewTaskForm workspace={workspace} />

      {tasks.isPending && <p className="text-sm text-muted-foreground">Loading tasks...</p>}
      {tasks.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load tasks: {tasks.error.message}
        </p>
      )}
      {tasks.data?.length === 0 && (
        <p className="text-sm text-muted-foreground">No tasks in this workspace yet.</p>
      )}
      {tasks.data && tasks.data.length > 0 && (
        <>
          <div role="group" aria-label="Task view" className="flex gap-1">
            {(["list", "timeline"] as const).map((option) => (
              <Button
                key={option}
                size="xs"
                variant={view === option ? "secondary" : "ghost"}
                aria-pressed={view === option}
                onClick={() => setView(option)}
              >
                {option === "list" ? "List" : "Timeline"}
              </Button>
            ))}
          </div>
          {view === "list" ? (
            <ul aria-label="Tasks" className="flex flex-col divide-y rounded-lg border">
              {tasks.data.map((task) => (
                <TaskRow key={task.id} task={task} tasks={tasks.data} />
              ))}
            </ul>
          ) : (
            <TaskGraph
              workspaceId={workspace.id}
              tasks={tasks.data}
              statusLabels={statusLabels}
              statusStyles={statusStyles}
            />
          )}
        </>
      )}
    </section>
  );
}

/** Statuses whose task can still be cancelled; `set_task_status` allows these. */
const cancellableStatuses: TaskStatus[] = ["planned", "ready", "in_progress", "in_review"];

/** Statuses whose task has no live worktree to lose, so it can be deleted. */
const deletableStatuses: TaskStatus[] = ["planned", "ready", "done", "cancelled"];

type Confirmation = "cancel" | "delete" | "remove-worktree" | "force-remove-worktree";

const confirmLabels: Record<Confirmation, string> = {
  cancel: "Confirm cancel",
  delete: "Confirm delete",
  "remove-worktree": "Confirm remove worktree",
  "force-remove-worktree": "Confirm force remove",
};

function TaskTimeline({ task }: { task: Task }) {
  const milestones = reachedMilestones(task.milestones);
  if (milestones.length === 0) {
    return null;
  }
  return (
    <p aria-label={`Timeline of #${task.id}`} className="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
      {milestones.map(({ label, at }) => (
        <time key={label} dateTime={new Date(at * 1000).toISOString()} title={new Date(at * 1000).toLocaleString()}>
          {`${label} ${formatDate(at)}`}
        </time>
      ))}
    </p>
  );
}

function TaskRow({ task, tasks }: { task: Task; tasks: Task[] }) {
  const herdr = useHerdrStatus();
  const agentStatuses = useAgentStatuses();
  const startTask = useStartTask();
  const openTask = useOpenTaskInHerdr();
  const refreshCommits = useRefreshCommitRecords();
  const openPullRequest = useOpenPullRequest();
  const completeTask = useCompleteTask();
  const cancelTask = useCancelTask();
  const deleteTask = useDeleteTask();
  const addDependency = useAddDependency();
  const removeDependency = useRemoveDependency();
  const removeWorktree = useRemoveTaskWorktree();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState<Confirmation | null>(null);
  const herdrRunning = herdr.data?.running === true;
  const inReview = task.status === "in_review";
  const active = task.status === "in_progress" || inReview;
  const hasWorktree = active && task.worktreePath !== null;
  const agentStatus = agentStatuses.data?.find((entry) => entry.taskId === task.id)?.status;
  const finishing = refreshCommits.isPending || openPullRequest.isPending || completeTask.isPending;
  const final = task.status === "done" || task.status === "cancelled";
  const removableWorktree = final && task.worktreePath !== null;
  // A rejected removal means the worktree has local changes; offer to discard them.
  const dirtyWorktree =
    removableWorktree && removeWorktree.error instanceof CommandFailure && removeWorktree.error.kind === "rejected";
  const dependenciesEditable = task.status === "planned" || task.status === "ready";
  const changingDependencies = addDependency.isPending || removeDependency.isPending;
  const ref = { workspaceId: task.workspaceId, id: task.id };
  const actionError =
    startTask.error ??
    openTask.error ??
    refreshCommits.error ??
    openPullRequest.error ??
    completeTask.error ??
    cancelTask.error ??
    deleteTask.error ??
    removeWorktree.error ??
    addDependency.error ??
    removeDependency.error;
  const candidates = tasks.filter(
    (other) => other.id !== task.id && other.status !== "cancelled" && !task.dependsOn.includes(other.id),
  );

  function confirm(action: Confirmation) {
    const onSettled = () => setConfirming(null);
    if (action === "remove-worktree" || action === "force-remove-worktree") {
      removeWorktree.mutate({ id: task.id, force: action === "force-remove-worktree" }, { onSettled });
    } else {
      (action === "cancel" ? cancelTask : deleteTask).mutate(ref, { onSettled });
    }
  }

  return (
    <li className="flex items-start gap-3 px-3 py-2">
      <span className="mt-0.5 w-8 shrink-0 text-xs tabular-nums text-muted-foreground">#{task.id}</span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {editing ? (
          <TaskEditForm task={task} onDone={() => setEditing(false)} />
        ) : (
          <>
            <div className="flex items-center gap-2">
              <span className="rounded-md border px-1.5 text-xs font-medium">{task.taskType}</span>
              <span className="truncate text-sm font-medium">{task.title}</span>
            </div>
            {task.description && (
              <p className="line-clamp-2 text-xs text-muted-foreground">{task.description}</p>
            )}
          </>
        )}
        {(task.dependsOn.length > 0 || (dependenciesEditable && candidates.length > 0)) && (
          <div
            role="group"
            aria-label={`Dependencies of #${task.id}`}
            className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground"
          >
            {task.dependsOn.length > 0 && <span>Depends on</span>}
            {task.dependsOn.map((id) => (
              <span key={id} className="inline-flex items-center gap-0.5 rounded-md border px-1 tabular-nums">
                <span>#{id}</span>
                {dependenciesEditable && (
                  <button
                    type="button"
                    aria-label={`Remove dependency #${id} from #${task.id}`}
                    className="px-0.5 hover:text-foreground disabled:opacity-50"
                    disabled={changingDependencies}
                    onClick={() => removeDependency.mutate({ ...ref, dependsOn: id })}
                  >
                    {"\u00d7"}
                  </button>
                )}
              </span>
            ))}
            {dependenciesEditable && candidates.length > 0 && (
              <select
                aria-label={`Add dependency to #${task.id}`}
                className="h-6 max-w-48 rounded-md border bg-transparent px-1 text-xs"
                value=""
                disabled={changingDependencies}
                onChange={(e) => {
                  if (e.target.value) {
                    addDependency.mutate({ ...ref, dependsOn: Number(e.target.value) });
                  }
                }}
              >
                <option value="">Add dependency...</option>
                {candidates.map((other) => (
                  <option key={other.id} value={other.id}>
                    #{other.id} {other.title}
                  </option>
                ))}
              </select>
            )}
          </div>
        )}
        <TaskTimeline task={task} />
        {task.branch && (
          <p className="truncate font-mono text-xs text-muted-foreground" title={task.worktreePath ?? undefined}>
            {task.branch}
            {task.worktreePath && ` \u00b7 ${task.worktreePath}`}
          </p>
        )}
        {task.pullRequestUrl && (
          <p className="truncate text-xs">
            <span className="text-muted-foreground">Pull request </span>
            <span className="select-all font-mono">{task.pullRequestUrl}</span>
          </p>
        )}
        <CommitList task={task} />
        {actionError && (
          <p role="alert" className="text-xs text-destructive">
            {actionError.message}
          </p>
        )}
        {!editing && (
          <div className="mt-0.5 flex flex-wrap gap-1">
            {confirming ? (
              <>
                <Button
                  size="xs"
                  variant="destructive"
                  aria-label={`${confirmLabels[confirming]} #${task.id}`}
                  disabled={cancelTask.isPending || deleteTask.isPending || removeWorktree.isPending}
                  onClick={() => confirm(confirming)}
                >
                  {removeWorktree.isPending ? "Removing..." : confirmLabels[confirming]}
                </Button>
                <Button size="xs" variant="ghost" onClick={() => setConfirming(null)}>
                  Back
                </Button>
              </>
            ) : (
              <>
                {!final && (
                  <Button size="xs" variant="ghost" aria-label={`Edit #${task.id}`} onClick={() => setEditing(true)}>
                    Edit
                  </Button>
                )}
                {cancellableStatuses.includes(task.status) && (
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Cancel #${task.id}`}
                    onClick={() => setConfirming("cancel")}
                  >
                    Cancel
                  </Button>
                )}
                {removableWorktree && (
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Remove worktree of #${task.id}`}
                    title="Remove the worktree and close its herdr workspace; the branch is kept"
                    disabled={!herdrRunning}
                    onClick={() => setConfirming("remove-worktree")}
                  >
                    Remove worktree
                  </Button>
                )}
                {dirtyWorktree && (
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Force remove worktree of #${task.id}`}
                    title="Remove the worktree and discard its modified and untracked files"
                    disabled={!herdrRunning}
                    onClick={() => setConfirming("force-remove-worktree")}
                  >
                    Force remove
                  </Button>
                )}
                {deletableStatuses.includes(task.status) && (
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Delete #${task.id}`}
                    onClick={() => setConfirming("delete")}
                  >
                    Delete
                  </Button>
                )}
              </>
            )}
          </div>
        )}
      </div>
      {task.status === "ready" && (
        <Button
          size="xs"
          aria-label={`Start #${task.id}`}
          disabled={!herdrRunning || startTask.isPending}
          onClick={() => startTask.mutate(task.id)}
        >
          {startTask.isPending ? "Starting..." : "Start"}
        </Button>
      )}
      {active && task.herdrWorkspaceId && (
        <Button
          size="xs"
          variant="outline"
          aria-label={`Open #${task.id} in herdr`}
          disabled={!herdrRunning || openTask.isPending}
          onClick={() => openTask.mutate(task.id)}
        >
          Open in herdr
        </Button>
      )}
      {hasWorktree && (
        <>
          <Button
            size="xs"
            variant="outline"
            aria-label={`Refresh commits of #${task.id}`}
            disabled={finishing}
            onClick={() => refreshCommits.mutate(task.id)}
          >
            {refreshCommits.isPending ? "Refreshing..." : "Refresh commits"}
          </Button>
          <Button
            size="xs"
            variant={inReview ? "outline" : "default"}
            aria-label={inReview ? `Push #${task.id} to its pull request` : `Open pull request for #${task.id}`}
            disabled={finishing}
            onClick={() => openPullRequest.mutate(task.id)}
          >
            {openPullRequest.isPending ? "Pushing..." : inReview ? "Push to PR" : "Open PR"}
          </Button>
        </>
      )}
      {inReview && (
        <Button
          size="xs"
          aria-label={`Complete #${task.id}`}
          title="Complete the task once its pull request is merged; its herdr workspace is closed and the worktree kept"
          disabled={finishing || (task.herdrWorkspaceId !== null && !herdrRunning)}
          onClick={() => completeTask.mutate(task.id)}
        >
          {completeTask.isPending ? "Completing..." : "Complete"}
        </Button>
      )}
      {active && agentStatus && (
        <span
          aria-label={`Agent of #${task.id}`}
          className={cn("shrink-0 rounded-full px-2 py-0.5 text-xs font-medium", agentStyles[agentStatus])}
        >
          {agentLabels[agentStatus]}
        </span>
      )}
      <span className={cn("shrink-0 rounded-full px-2 py-0.5 text-xs font-medium", statusStyles[task.status])}>
        {statusLabels[task.status]}
      </span>
    </li>
  );
}

function TaskEditForm({ task, onDone }: { task: Task; onDone: () => void }) {
  const updateTask = useUpdateTask();
  const [taskType, setTaskType] = useState<TaskType>(task.taskType);
  const [title, setTitle] = useState(task.title);
  const [description, setDescription] = useState(task.description);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    updateTask.mutate(
      { id: task.id, taskType, title: title.trim(), description: description.trim() },
      { onSuccess: onDone },
    );
  }

  return (
    <form onSubmit={submit} aria-label={`Edit task #${task.id}`} className="flex flex-col gap-1">
      <div className="flex gap-2">
        <select
          aria-label={`Type of #${task.id}`}
          className={cn(inputClass, "w-28")}
          value={taskType}
          onChange={(e) => setTaskType(e.target.value as TaskType)}
        >
          {taskTypes.map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
        </select>
        <input
          className={inputClass}
          aria-label={`Title of #${task.id}`}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          required
        />
      </div>
      <input
        className={inputClass}
        placeholder="Description (optional)"
        aria-label={`Description of #${task.id}`}
        value={description}
        onChange={(e) => setDescription(e.target.value)}
      />
      <div className="flex gap-1">
        <Button type="submit" size="xs" disabled={updateTask.isPending}>
          {updateTask.isPending ? "Saving..." : "Save"}
        </Button>
        <Button type="button" size="xs" variant="ghost" onClick={onDone}>
          Discard
        </Button>
      </div>
      {updateTask.isError && (
        <p role="alert" className="text-xs text-destructive">
          {updateTask.error.message}
        </p>
      )}
    </form>
  );
}

function CommitList({ task }: { task: Task }) {
  const recorded = task.status === "in_progress" || task.status === "in_review" || task.status === "done";
  const commits = useCommitRecords(task.id, recorded);

  if (commits.isError) {
    return (
      <p role="alert" className="text-xs text-destructive">
        Failed to load commits: {commits.error.message}
      </p>
    );
  }
  if (!commits.data || commits.data.length === 0) {
    return null;
  }
  return (
    <ul aria-label={`Commits of #${task.id}`} className="mt-1 flex flex-col gap-0.5">
      {commits.data.map((commit) => (
        <li key={commit.sha} className="flex gap-2 text-xs" title={commit.author}>
          <code className="shrink-0 text-muted-foreground">{commit.sha.slice(0, 7)}</code>
          <span className="truncate">{commit.message.split("\n", 1)[0]}</span>
        </li>
      ))}
    </ul>
  );
}

function WorktreeRootForm({ workspace }: { workspace: Workspace }) {
  const machine = machineOf(workspace.location);
  const settings = useMachineSettings(machine);
  const saveSettings = useSaveMachineSettings();
  const [draft, setDraft] = useState<string | null>(null);
  const saved = settings.data?.worktreeRoot ?? "";
  const machineName = machine.kind === "local" ? "this computer" : machine.label;

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    saveSettings.mutate(
      { machine, worktreeRoot: (draft ?? saved).trim() || null },
      { onSuccess: () => setDraft(null) },
    );
  }

  return (
    <form onSubmit={submit} aria-label="Worktree root" className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <label htmlFor="worktree-root" className="shrink-0 text-xs text-muted-foreground">
          Worktree root on {machineName}
        </label>
        <input
          id="worktree-root"
          className={cn(inputClass, "font-mono")}
          placeholder={machine.kind === "local" ? "D:\\worktrees" : "/srv/worktrees"}
          value={draft ?? saved}
          onChange={(e) => setDraft(e.target.value)}
          disabled={!settings.isSuccess}
        />
        <Button type="submit" size="sm" variant="outline" disabled={draft === null || saveSettings.isPending}>
          Save
        </Button>
      </div>
      {settings.isSuccess && !saved && (
        <p className="text-xs text-muted-foreground">Set a worktree root to start tasks on this machine.</p>
      )}
      {(settings.error ?? saveSettings.error) && (
        <p role="alert" className="text-xs text-destructive">
          {(settings.error ?? saveSettings.error)?.message}
        </p>
      )}
    </form>
  );
}

function NewTaskForm({ workspace }: { workspace: Workspace }) {
  const createTask = useCreateTask();
  const [taskType, setTaskType] = useState<TaskType>("feat");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    createTask.mutate(
      { workspaceId: workspace.id, taskType, title, description },
      {
        onSuccess: () => {
          setTitle("");
          setDescription("");
        },
      },
    );
  }

  return (
    <form onSubmit={submit} aria-label="New task" className="flex flex-col gap-2">
      <div className="flex gap-2">
        <select
          aria-label="Task type"
          className={cn(inputClass, "w-28")}
          value={taskType}
          onChange={(e) => setTaskType(e.target.value as TaskType)}
        >
          {taskTypes.map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
        </select>
        <input
          className={inputClass}
          placeholder="Task title"
          aria-label="Task title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          required
        />
        <Button type="submit" disabled={createTask.isPending}>
          Add task
        </Button>
      </div>
      <input
        className={inputClass}
        placeholder="Description (optional)"
        aria-label="Description"
        value={description}
        onChange={(e) => setDescription(e.target.value)}
      />
      {createTask.isError && (
        <p role="alert" className="text-xs text-destructive">
          {createTask.error.message}
        </p>
      )}
    </form>
  );
}
