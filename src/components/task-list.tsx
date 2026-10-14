import { useState, type FormEvent, type ReactNode } from "react";
import {
  Check,
  FolderGit2,
  GitBranch,
  GitPullRequest,
  Inbox,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  SquareTerminal,
  StickyNote,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { TaskGraph } from "@/components/task-graph";
import { compactSelectClass, inputClass, describeLocation } from "@/components/workspace-list";
import type { AgentStatus, Task, TaskId, TaskStatus, TaskType, Workspace, WorkspaceId } from "@/lib/bindings";
import {
  CommandFailure,
  machineOf,
  useAddDependency,
  useAgentStatuses,
  useCommitRecords,
  useCompleteTask,
  useCreateTask,
  useDeleteTask,
  useHerdrStatus,
  useMachineSettings,
  useOpenPullRequest,
  useOpenTaskInHerdr,
  usePauseTask,
  useRefreshCommitRecords,
  useRemoveDependency,
  useRestoreTaskWorktree,
  useSaveMachineSettings,
  useSetCompletionNote,
  useStartTask,
  useTasks,
  useUpdateTask,
  useUpdateWorkspace,
  useWorkspaces,
} from "@/lib/queries";
import { pullRequestStats } from "@/lib/review";
import { dependenciesDone, isPaused } from "@/lib/task-graph";
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
  working: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200",
  blocked: "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
};

const statusLabels: Record<TaskStatus, string> = {
  planned: "Planned",
  in_progress: "In progress",
  in_review: "In review",
  done: "Done",
};

/** The status of `task`, naming a planned task that was worked on before "Paused". */
function statusLabel(task: Task): string {
  return isPaused(task) ? "Paused" : statusLabels[task.status];
}

const statusStyles: Record<TaskStatus, string> = {
  planned: "bg-muted text-muted-foreground",
  in_progress: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-200",
  in_review: "bg-violet-100 text-violet-800 dark:bg-violet-950 dark:text-violet-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
};

/** What the task list needs to name tasks of other workspaces. */
type TaskContext = {
  /** Tasks of every workspace, by ID. */
  byId: ReadonlyMap<TaskId, Task>;
  workspaceName: (id: WorkspaceId) => string;
};

/** `#id`, prefixed with the workspace name when the task belongs to another workspace than `of`. */
function taskRef(id: TaskId, of: WorkspaceId, { byId, workspaceName }: TaskContext): string {
  const other = byId.get(id);
  return other === undefined || other.workspaceId === of ? `#${id}` : `${workspaceName(other.workspaceId)} #${id}`;
}

/**
 * Tasks of `workspace`, or of every workspace when it is null. The dependency DAG spans
 * workspaces, so a task may wait on a task of another workspace. The timeline is only
 * offered for every workspace, where it is the default view.
 */
export function TaskList({ workspace }: { workspace: Workspace | null }) {
  const allTasks = useTasks();
  const workspaces = useWorkspaces();
  const [view, setView] = useState<"list" | "timeline">("timeline");
  const [timelineWorkspace, setTimelineWorkspace] = useState<WorkspaceId | null>(null);
  const names = new Map(workspaces.data?.map((entry) => [entry.id, entry.name]));
  const context: TaskContext = {
    byId: new Map(allTasks.data?.map((task) => [task.id, task])),
    workspaceName: (id) => names.get(id) ?? `Workspace ${id}`,
  };
  const tasks =
    workspace === null ? allTasks.data : allTasks.data?.filter((task) => task.workspaceId === workspace.id);
  const showTimeline = workspace === null && view === "timeline";
  // Workspaces the timeline can be narrowed to; a filter whose workspace has no tasks left falls back to all.
  const filterOptions = [...new Set(allTasks.data?.map((task) => task.workspaceId))].sort((a, b) =>
    context.workspaceName(a).localeCompare(context.workspaceName(b)),
  );
  const filter = timelineWorkspace !== null && filterOptions.includes(timelineWorkspace) ? timelineWorkspace : null;
  const filtered = filter === null ? tasks : tasks?.filter((task) => task.workspaceId === filter);
  // Prerequisites in other workspaces are drawn too, dimmed, so the wait is visible.
  const prerequisites =
    filter === null
      ? []
      : (tasks ?? []).filter(
          (task) => task.workspaceId !== filter && filtered?.some((own) => own.dependsOn.includes(task.id)),
        );

  return (
    <section aria-labelledby="tasks-heading" className="flex flex-col gap-4">
      <header>
        <h2 id="tasks-heading" className="text-xl font-semibold tracking-tight">
          {workspace?.name ?? "All workspaces"}
        </h2>
        {workspace ? (
          <BaseBranchForm workspace={workspace} />
        ) : (
          <p className="text-sm text-muted-foreground">Tasks of every workspace and the dependencies between them.</p>
        )}
      </header>

      {workspace && <WorktreeRootForm workspace={workspace} />}

      {workspace && <NewTaskForm workspace={workspace} />}

      {allTasks.isPending && <p className="text-sm text-muted-foreground">Loading tasks...</p>}
      {allTasks.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load tasks: {allTasks.error.message}
        </p>
      )}
      {tasks?.length === 0 && (
        <EmptyState
          title={workspace ? "No tasks in this workspace yet." : "No tasks yet."}
          hint={
            workspace
              ? "Plan the first task above; tasks can depend on each other across workspaces."
              : "Pick a workspace in the sidebar to plan its tasks."
          }
        />
      )}
      {allTasks.data && tasks && tasks.length > 0 && (
        <>
          {workspace === null && (
            <div className="flex flex-wrap items-center gap-2">
              <div role="group" aria-label="Task view" className="flex gap-1">
                {(["timeline", "list"] as const).map((option) => (
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
              {showTimeline && (
                <select
                  aria-label="Timeline workspace"
                  className={compactSelectClass}
                  value={filter ?? "all"}
                  onChange={(event) =>
                    setTimelineWorkspace(event.target.value === "all" ? null : Number(event.target.value))
                  }
                >
                  <option value="all">All workspaces</option>
                  {filterOptions.map((id) => (
                    <option key={id} value={id}>
                      {context.workspaceName(id)}
                    </option>
                  ))}
                </select>
              )}
            </div>
          )}
          {!showTimeline ? (
            <ul aria-label="Tasks" className="flex flex-col divide-y rounded-xl border bg-card shadow-xs">
              {tasks.map((task) => (
                <TaskRow
                  key={task.id}
                  task={task}
                  tasks={allTasks.data}
                  context={context}
                  showWorkspace={workspace === null}
                />
              ))}
            </ul>
          ) : (
            <TaskGraph
              tasks={[...(filtered ?? []), ...prerequisites]}
              allTasks={allTasks.data}
              dimmed={new Set(prerequisites.map((task) => task.id))}
              workspaceLabel={(task) =>
                filter === null || task.workspaceId !== filter ? context.workspaceName(task.workspaceId) : null
              }
              statusLabel={statusLabel}
              statusStyles={statusStyles}
            />
          )}
        </>
      )}
    </section>
  );
}

/** Statuses whose task is not being worked on, so it can be deleted with its worktree. */
const deletableStatuses: TaskStatus[] = ["planned", "done"];

type Confirmation = "delete" | "force-delete";

const confirmLabels: Record<Confirmation, string> = {
  delete: "Confirm delete",
  "force-delete": "Confirm force delete",
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

function TaskRow({
  task,
  tasks,
  context,
  showWorkspace,
}: {
  task: Task;
  /** Tasks of every workspace; any of them can become a prerequisite. */
  tasks: Task[];
  context: TaskContext;
  showWorkspace: boolean;
}) {
  const herdr = useHerdrStatus();
  const agentStatuses = useAgentStatuses();
  const startTask = useStartTask();
  const openTask = useOpenTaskInHerdr();
  const refreshCommits = useRefreshCommitRecords();
  const openPullRequest = useOpenPullRequest();
  const completeTask = useCompleteTask();
  const pauseTask = usePauseTask();
  const deleteTask = useDeleteTask();
  const addDependency = useAddDependency();
  const removeDependency = useRemoveDependency();
  const restoreWorktree = useRestoreTaskWorktree();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState<Confirmation | null>(null);
  const herdrRunning = herdr.data?.running === true;
  const inReview = task.status === "in_review";
  const active = task.status === "in_progress" || inReview;
  const hasWorktree = active && task.worktreePath !== null;
  const agentStatus = agentStatuses.data?.find((entry) => entry.taskId === task.id)?.status;
  const finishing = refreshCommits.isPending || openPullRequest.isPending || completeTask.isPending;
  const final = task.status === "done";
  const paused = isPaused(task);
  const startable = task.status === "planned" && dependenciesDone(task, context.byId);
  // A paused task may still have the worktree it had while it was worked on.
  const removableWorktree = !active && task.worktreePath !== null;
  // A rejected delete means the worktree has local changes; offer to discard them.
  const dirtyWorktree =
    removableWorktree && deleteTask.error instanceof CommandFailure && deleteTask.error.kind === "rejected";
  const dependenciesEditable = task.status === "planned";
  const changingDependencies = addDependency.isPending || removeDependency.isPending;
  const actionError =
    startTask.error ??
    openTask.error ??
    refreshCommits.error ??
    openPullRequest.error ??
    completeTask.error ??
    pauseTask.error ??
    deleteTask.error ??
    restoreWorktree.error ??
    addDependency.error ??
    removeDependency.error;
  const candidates = tasks.filter(
    (other) => other.id !== task.id && !task.dependsOn.includes(other.id),
  );
  // Candidates grouped by workspace, the task's own workspace first.
  const candidateGroups = [...new Set([task.workspaceId, ...candidates.map((other) => other.workspaceId)])]
    .map((workspaceId) => ({
      workspaceId,
      tasks: candidates.filter((other) => other.workspaceId === workspaceId),
    }))
    .filter((group) => group.tasks.length > 0);

  function confirm(action: Confirmation) {
    deleteTask.mutate({ id: task.id, force: action === "force-delete" }, { onSettled: () => setConfirming(null) });
  }

  return (
    <li className="flex flex-col gap-1.5 px-4 py-3">
      {editing ? (
        <TaskEditForm task={task} onDone={() => setEditing(false)} />
      ) : (
        <div className="flex min-w-0 flex-col gap-0.5">
          <div className="flex min-w-0 items-center gap-2">
            <StatusBadge task={task} />
            <span className="shrink-0 text-xs tabular-nums text-muted-foreground">#{task.id}</span>
            {showWorkspace && (
              <span className="shrink-0 rounded-md bg-muted px-1.5 text-xs text-muted-foreground">
                {context.workspaceName(task.workspaceId)}
              </span>
            )}
            <span className="shrink-0 rounded-md border px-1.5 font-mono text-xs">{task.taskType}</span>
            <span className="min-w-0 flex-1 truncate text-sm font-medium" title={task.title}>
              {task.title}
            </span>
            {final && task.completionNote === null && (
              <span
                aria-label={`#${task.id} needs a completion note`}
                title="Write what this task achieved; reviews include the note"
                className="shrink-0 rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-900 dark:bg-amber-950 dark:text-amber-200"
              >
                Needs note
              </span>
            )}
            {active && agentStatus && (
              <span
                aria-label={`Agent of #${task.id}`}
                className={cn(
                  "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium",
                  agentStyles[agentStatus],
                )}
              >
                <span
                  aria-hidden
                  className={cn("size-1.5 rounded-full bg-current", agentStatus === "working" && "animate-pulse")}
                />
                {agentLabels[agentStatus]}
              </span>
            )}
          </div>
          {task.description && <p className="line-clamp-2 text-xs text-muted-foreground">{task.description}</p>}
        </div>
      )}
      {(task.dependsOn.length > 0 || (dependenciesEditable && candidates.length > 0)) && (
        <div
          role="group"
          aria-label={`Dependencies of #${task.id}`}
          className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground"
        >
          {task.dependsOn.length > 0 && <span>Depends on</span>}
          {task.dependsOn.map((id) => (
            <span
              key={id}
              className={cn(
                "inline-flex h-6 items-center gap-0.5 rounded-md border px-1.5 tabular-nums",
                context.byId.get(id)?.status === "done" && "line-through decoration-muted-foreground/60",
              )}
              title={context.byId.get(id)?.title}
            >
              <span>{taskRef(id, task.workspaceId, context)}</span>
              {dependenciesEditable && (
                <button
                  type="button"
                  aria-label={`Remove dependency #${id} from #${task.id}`}
                  className="rounded-sm p-0.5 hover:bg-muted hover:text-foreground disabled:opacity-50"
                  disabled={changingDependencies}
                  onClick={() => removeDependency.mutate({ id: task.id, dependsOn: id })}
                >
                  <X className="size-3" />
                </button>
              )}
            </span>
          ))}
          {dependenciesEditable && candidates.length > 0 && (
            <select
              aria-label={`Add dependency to #${task.id}`}
              className={cn(compactSelectClass, "max-w-48")}
              value=""
              disabled={changingDependencies}
              onChange={(e) => {
                if (e.target.value) {
                  addDependency.mutate({ id: task.id, dependsOn: Number(e.target.value) });
                }
              }}
            >
              <option value="">Add dependency...</option>
              {candidateGroups.map((group) => (
                <optgroup key={group.workspaceId} label={context.workspaceName(group.workspaceId)}>
                  {group.tasks.map((other) => (
                    <option key={other.id} value={other.id}>
                      #{other.id} {other.title}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          )}
        </div>
      )}
      <TaskTimeline task={task} />
      {task.branch && (
        <p
          className="flex min-w-0 items-center gap-1 font-mono text-xs text-muted-foreground"
          title={task.worktreePath ?? undefined}
        >
          <GitBranch aria-hidden className="size-3 shrink-0" />
          <span className="truncate">
            {task.branch}
            {task.worktreePath && ` \u00b7 ${task.worktreePath}`}
          </span>
        </p>
      )}
      {task.pullRequestUrl && (
        <p className="flex min-w-0 items-center gap-1 text-xs">
          <GitPullRequest aria-hidden className="size-3 shrink-0 text-muted-foreground" />
          <span className="sr-only">Pull request </span>
          <span className="truncate select-all font-mono">{task.pullRequestUrl}</span>
          {task.pullRequest && (
            <span aria-label={`Pull request of #${task.id}`} className="shrink-0 text-muted-foreground">
              {pullRequestStats(task.pullRequest)}
            </span>
          )}
        </p>
      )}
      <CommitList task={task} />
      {final && !editing && <CompletionNote task={task} />}
      {actionError && (
        <p role="alert" className="text-xs text-destructive">
          {actionError.message}
        </p>
      )}
      {!editing && (
        <div className="flex flex-wrap items-center gap-1">
          {task.status === "planned" && (
            <Button
              size="xs"
              aria-label={`${paused ? "Resume" : "Start"} #${task.id}`}
              title={startable ? undefined : "Waits until all its dependencies are done"}
              disabled={!startable || !herdrRunning || startTask.isPending}
              onClick={() => startTask.mutate(task.id)}
            >
              <Play />
              {startTask.isPending ? "Starting..." : paused ? "Resume" : "Start"}
            </Button>
          )}
          {active && task.worktreePath === null && task.branch !== null && (
            <Button
              size="xs"
              variant="outline"
              aria-label={`Restore worktree of #${task.id}`}
              title={`Check out ${task.branch} into a new worktree and open it in herdr`}
              disabled={!herdrRunning || restoreWorktree.isPending}
              onClick={() => restoreWorktree.mutate(task.id)}
            >
              <FolderGit2 />
              {restoreWorktree.isPending ? "Restoring..." : "Restore worktree"}
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
              <SquareTerminal />
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
                <RefreshCw className={cn(refreshCommits.isPending && "animate-spin")} />
                {refreshCommits.isPending ? "Refreshing..." : "Refresh commits"}
              </Button>
              <Button
                size="xs"
                variant={inReview ? "outline" : "default"}
                aria-label={inReview ? `Push #${task.id} to its pull request` : `Open pull request for #${task.id}`}
                disabled={finishing}
                onClick={() => openPullRequest.mutate(task.id)}
              >
                {inReview ? <Upload /> : <GitPullRequest />}
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
              <Check />
              {completeTask.isPending ? "Completing..." : "Complete"}
            </Button>
          )}
          <div className="ml-auto flex items-center gap-0.5">
            {confirming ? (
              <>
                <Button
                  size="xs"
                  variant="destructive"
                  aria-label={`${confirmLabels[confirming]} #${task.id}`}
                  disabled={deleteTask.isPending}
                  onClick={() => confirm(confirming)}
                >
                  <Trash2 />
                  {deleteTask.isPending ? "Deleting..." : confirmLabels[confirming]}
                </Button>
                <Button size="xs" variant="ghost" onClick={() => setConfirming(null)}>
                  Back
                </Button>
              </>
            ) : (
              <>
                {!final && (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={`Edit #${task.id}`}
                    title="Edit"
                    onClick={() => setEditing(true)}
                  >
                    <Pencil />
                  </Button>
                )}
                {active && (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={`Pause #${task.id}`}
                    title="Pause: stop working on the task; it keeps its branch, worktree, and pull request and can be resumed"
                    disabled={pauseTask.isPending}
                    onClick={() => pauseTask.mutate(task.id)}
                  >
                    <Pause />
                  </Button>
                )}
                {dirtyWorktree && (
                  <Button
                    size="xs"
                    variant="destructive"
                    aria-label={`Force delete #${task.id}`}
                    title="Delete the task and discard the modified and untracked files of its worktree"
                    disabled={!herdrRunning}
                    onClick={() => setConfirming("force-delete")}
                  >
                    Force delete
                  </Button>
                )}
                {deletableStatuses.includes(task.status) && (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={`Delete #${task.id}`}
                    title={
                      removableWorktree
                        ? "Delete the task and remove its worktree and herdr workspace; the branch is kept"
                        : "Delete"
                    }
                    className="hover:text-destructive"
                    disabled={removableWorktree && !herdrRunning}
                    onClick={() => setConfirming("delete")}
                  >
                    <Trash2 />
                  </Button>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </li>
  );
}

/** The status of `task` as a pill with a colored dot. */
function StatusBadge({ task }: { task: Task }) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium",
        statusStyles[task.status],
      )}
    >
      <span aria-hidden className="size-1.5 rounded-full bg-current opacity-80" />
      {statusLabel(task)}
    </span>
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

/** The completion note of a done task, which reviews include, or a button to write one. */
function CompletionNote({ task }: { task: Task }) {
  const setNote = useSetCompletionNote();
  const [editing, setEditing] = useState(false);
  const [note, setNoteText] = useState("");
  const saved = task.completionNote ?? "";

  function edit() {
    setNoteText(saved);
    setEditing(true);
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setNote.mutate({ id: task.id, note }, { onSuccess: () => setEditing(false) });
  }

  if (!editing) {
    return task.completionNote === null ? (
      <Button
        size="xs"
        variant="outline"
        className="self-start"
        aria-label={`Add completion note to #${task.id}`}
        onClick={edit}
      >
        <StickyNote />
        Add note
      </Button>
    ) : (
      <div className="flex items-start gap-1 rounded-md bg-muted/60 px-2 py-1">
        <StickyNote aria-hidden className="mt-0.5 size-3 shrink-0 text-muted-foreground" />
        <p aria-label={`Completion note of #${task.id}`} className="min-w-0 flex-1 whitespace-pre-line text-xs">
          {task.completionNote}
        </p>
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={`Edit completion note of #${task.id}`}
          title="Edit note"
          onClick={edit}
        >
          <Pencil />
        </Button>
      </div>
    );
  }
  return (
    <form onSubmit={submit} aria-label={`Completion note form of #${task.id}`} className="flex flex-col gap-1">
      <textarea
        className={cn(inputClass, "h-auto min-h-16 py-1.5")}
        aria-label={`Completion note for #${task.id}`}
        placeholder="What did this achieve? Impact, decisions, follow-ups. Reviews include this note."
        value={note}
        onChange={(e) => setNoteText(e.target.value)}
        autoFocus
      />
      <div className="flex gap-1">
        <Button type="submit" size="xs" disabled={setNote.isPending || note.trim() === saved}>
          {setNote.isPending ? "Saving..." : "Save note"}
        </Button>
        <Button type="button" size="xs" variant="ghost" onClick={() => setEditing(false)}>
          Discard
        </Button>
      </div>
      {setNote.isError && (
        <p role="alert" className="text-xs text-destructive">
          {setNote.error.message}
        </p>
      )}
    </form>
  );
}

function CommitList({ task }: { task: Task }) {
  const recorded = task.status !== "planned" || isPaused(task);
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

/** A dashed placeholder for a list with nothing in it yet. */
export function EmptyState({ title, hint, icon }: { title: string; hint?: string; icon?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-1 rounded-xl border border-dashed px-6 py-10 text-center">
      <span aria-hidden className="mb-1 text-muted-foreground [&_svg]:size-6">
        {icon ?? <Inbox />}
      </span>
      <p className="text-sm font-medium">{title}</p>
      {hint && <p className="max-w-sm text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
}

/**
 * The location and default base branch of the workspace; the branch that new task branches
 * start from and pull requests target can be changed in place.
 */
function BaseBranchForm({ workspace }: { workspace: Workspace }) {
  const updateWorkspace = useUpdateWorkspace();
  const tasks = useTasks();
  // An open pull request targets the current branch, so it stays while a task is in review.
  const inReview = tasks.data?.filter((task) => task.workspaceId === workspace.id && task.status === "in_review");
  const locked = inReview === undefined || inReview.length > 0;
  const [draft, setDraft] = useState<string | null>(null);
  const location = describeLocation(workspace.location);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (draft === null) return;
    updateWorkspace.mutate({ ...workspace, defaultBaseBranch: draft.trim() }, { onSuccess: () => setDraft(null) });
  }

  if (draft === null) {
    return (
      <p className="flex min-w-0 items-center gap-1 text-sm text-muted-foreground">
        <span className="truncate">{location} &middot; base</span>
        <code className="text-foreground">{workspace.defaultBaseBranch}</code>
        <Button
          size="xs"
          variant="ghost"
          aria-label="Change base branch"
          disabled={locked}
          onClick={() => {
            updateWorkspace.reset();
            setDraft(workspace.defaultBaseBranch);
          }}
        >
          Change
        </Button>
        {inReview && inReview.length > 0 && (
          <span className="truncate text-xs">
            Locked while {inReview.map((task) => `#${task.id}`).join(", ")} {inReview.length > 1 ? "are" : "is"} in
            review
          </span>
        )}
      </p>
    );
  }

  return (
    <form onSubmit={submit} aria-label="Base branch" className="flex flex-col gap-1.5">
      <div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
        <span className="truncate">{location} &middot;</span>
        <label htmlFor="base-branch" className="shrink-0">
          Base branch
        </label>
        <input
          id="base-branch"
          className={cn(inputClass, "max-w-56 font-mono")}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          autoFocus
        />
        <Button
          type="submit"
          size="sm"
          variant="outline"
          disabled={!draft.trim() || draft.trim() === workspace.defaultBaseBranch || updateWorkspace.isPending}
        >
          Save
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setDraft(null)}>
          Discard
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        New task branches start from this branch and pull requests target it.
      </p>
      {updateWorkspace.error && (
        <p role="alert" className="text-xs text-destructive">
          {updateWorkspace.error.message}
        </p>
      )}
    </form>
  );
}

/**
 * Where worktrees of the workspace's machine are created. Shown as a one-line summary once
 * set, and as an open form while unset or being changed.
 */
function WorktreeRootForm({ workspace }: { workspace: Workspace }) {
  const machine = machineOf(workspace.location);
  const settings = useMachineSettings(machine);
  const saveSettings = useSaveMachineSettings();
  const [draft, setDraft] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  const saved = settings.data?.worktreeRoot ?? "";
  const machineName = machine.kind === "local" ? "this computer" : machine.label;
  const error = settings.error ?? saveSettings.error;

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    saveSettings.mutate(
      { machine, worktreeRoot: (draft ?? saved).trim() || null },
      {
        onSuccess: () => {
          setDraft(null);
          setChanging(false);
        },
      },
    );
  }

  if (settings.isSuccess && saved && !changing) {
    return (
      <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
        <FolderGit2 aria-hidden className="size-3.5 shrink-0" />
        <span className="shrink-0">Worktrees on {machineName} in</span>
        <code className="truncate text-foreground" title={saved}>
          {saved}
        </code>
        <Button size="xs" variant="ghost" aria-label="Change worktree root" onClick={() => setChanging(true)}>
          Change
        </Button>
      </div>
    );
  }

  return (
    <form
      onSubmit={submit}
      aria-label="Worktree root"
      className={cn(
        "flex flex-col gap-1.5 rounded-xl border p-3",
        settings.isSuccess && !saved && "border-amber-300 bg-amber-50/60 dark:border-amber-900 dark:bg-amber-950/30",
      )}
    >
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
        {changing && (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            onClick={() => {
              setDraft(null);
              setChanging(false);
            }}
          >
            Discard
          </Button>
        )}
      </div>
      {settings.isSuccess && !saved && (
        <p className="text-xs text-muted-foreground">Set a worktree root to start tasks on this machine.</p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error.message}
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
    <form
      onSubmit={submit}
      aria-label="New task"
      className="flex flex-col gap-2 rounded-xl border bg-card p-3 shadow-xs"
    >
      <div className="flex gap-2">
        <select
          aria-label="Task type"
          className={cn(inputClass, "w-28 font-mono")}
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
          <Plus />
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
