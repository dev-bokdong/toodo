import { lazy, Suspense, useState, type FormEvent, type ReactNode } from "react";
import {
  Check,
  CircleDot,
  CircleSlash,
  FolderGit2,
  GitBranch,
  GitPullRequest,
  Inbox,
  Pause,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Search,
  SquareTerminal,
  StickyNote,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { TaskGraph } from "@/components/task-graph";
import { PrerequisiteConfirmation } from "@/components/prerequisite-confirmation";
import { compactSelectClass, inputClass, describeLocation } from "@/components/workspace-list";
import { AgentBadge } from "@/components/agent-badge";
import { PullRequestBadges } from "@/components/pull-request-badge";
import { TaskTypeBadge } from "@/components/task-type-badge";
import { MarkdownEditor } from "@/components/markdown-editor";
import type { NoteKind, Task, TaskId, TaskStatus, TaskType, Workspace, WorkspaceId } from "@/lib/bindings";
import {
  CommandFailure,
  IssueLinkFailure,
  machineOf,
  useAddDependency,
  useAgentStatuses,
  useCloseTaskWorkspace,
  useBranchFreshness,
  usePullRequestStatuses,
  useCommitRecords,
  useCompleteTask,
  useConcludeTask,
  useCreateBaseBranch,
  useCreateIssue,
  useCreateTask,
  useDeleteTask,
  useHerdrStatus,
  useLinkIssue,
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
  useWorkspaceBranches,
  useWorkspaces,
} from "@/lib/queries";
import { pullRequestStats } from "@/lib/review";
import {
  dependenciesDone,
  dependencyEditableStatuses,
  isNotAdopted,
  isPaused,
  pausesStartedTask,
  type TimelineScale,
} from "@/lib/task-graph";
import {
  emptyTaskFilter,
  isFilterActive,
  matchesFilter,
  shownStatus,
  shownStatuses,
  type ShownStatus,
  type TaskFilter,
} from "@/lib/task-filter";
import { formatDate, reachedMilestones } from "@/lib/timeline";
import { markdownPreview } from "@/lib/markdown";
import { cn } from "@/lib/utils";

const Markdown = lazy(() => import("@/components/markdown"));

const taskTypes: TaskType[] = ["feat", "fix", "modify", "refactor", "docs", "test", "chore", "rnd"];

const statusLabels: Record<ShownStatus, string> = {
  planned: "Planned",
  ready: "Ready",
  paused: "Paused",
  in_progress: "In progress",
  in_review: "In review",
  done: "Done",
  not_adopted: "Not adopted",
};

/**
 * The status of `task`, naming a planned task that can be started now "Ready", one that was
 * worked on before "Paused" and a task concluded without merging "Not adopted".
 */
function statusLabel(task: Task, byId: ReadonlyMap<TaskId, Task>): string {
  return statusLabels[shownStatus(task, byId)];
}

const statusStyles: Record<TaskStatus, string> = {
  planned: "bg-muted text-muted-foreground",
  in_progress: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-200",
  in_review: "bg-violet-100 text-violet-800 dark:bg-violet-950 dark:text-violet-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
};

const notAdoptedStyle = "bg-rose-100 text-rose-800 dark:bg-rose-950 dark:text-rose-200";

const readyStyle = "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200";

function statusStyle(task: Task, byId: ReadonlyMap<TaskId, Task>): string {
  switch (shownStatus(task, byId)) {
    case "not_adopted":
      return notAdoptedStyle;
    case "ready":
      return readyStyle;
    default:
      return statusStyles[task.status];
  }
}

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
export function TaskList({
  workspace,
  timelineScale,
  onTimelineScaleChange,
  draft = emptyDraft,
  onDraftChange,
}: {
  workspace: Workspace | null;
  /** Timeline scale, owned by the caller so it survives switching workspaces and views. */
  timelineScale: TimelineScale;
  onTimelineScaleChange: (scale: TimelineScale) => void;
  /** The new task being written in this workspace, owned by the caller so it survives switching away. */
  draft?: TaskDraft;
  onDraftChange?: (draft: TaskDraft) => void;
}) {
  const allTasks = useTasks();
  const workspaces = useWorkspaces();
  const [view, setView] = useState<"list" | "timeline">("timeline");
  const [timelineWorkspace, setTimelineWorkspace] = useState<WorkspaceId | null>(null);
  const [taskFilter, setTaskFilter] = useState<TaskFilter>(emptyTaskFilter);
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
  const filtered = tasks?.filter(
    (task) => (!showTimeline || filter === null || task.workspaceId === filter) && matchesFilter(task, taskFilter, context.byId),
  );
  // Prerequisites in other workspaces are drawn too, dimmed, so the wait is visible.
  const prerequisites =
    filter === null
      ? []
      : (tasks ?? []).filter(
          (task) => task.workspaceId !== filter && filtered?.some((own) => own.dependsOn.includes(task.id)),
        );

  return (
    <section aria-labelledby="tasks-heading" className="flex flex-1 flex-col gap-4">
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

      {workspace && onDraftChange && <NewTaskForm workspace={workspace} draft={draft} onChange={onDraftChange} />}

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
      {allTasks.data && tasks && filtered && tasks.length > 0 && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {workspace === null && (
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
            )}
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
            <TaskFilterBar filter={taskFilter} onChange={setTaskFilter} />
          </div>
          {filtered.length === 0 ? (
            <EmptyState
              icon={<Search />}
              title="No tasks match the search and filters."
              hint="Change the search text, status, or type, or clear the filters."
            />
          ) : !showTimeline ? (
            <ul aria-label="Tasks" className="flex flex-col divide-y rounded-xl border bg-card shadow-xs">
              {filtered.map((task) => (
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
              tasks={[...filtered, ...prerequisites]}
              allTasks={allTasks.data}
              dimmed={new Set(prerequisites.map((task) => task.id))}
              workspaceLabel={(task) =>
                filter === null || task.workspaceId !== filter ? context.workspaceName(task.workspaceId) : null
              }
              statusLabel={(task) => statusLabel(task, context.byId)}
              statusStyle={(task) => statusStyle(task, context.byId)}
              ready={(task) => shownStatus(task, context.byId) === "ready"}
              scale={timelineScale}
              onScaleChange={onTimelineScaleChange}
              renderDetail={(selected) => (
                <TaskRow
                  key={selected.id}
                  task={selected}
                  tasks={allTasks.data ?? []}
                  context={context}
                  showWorkspace
                  detailed
                />
              )}
            />
          )}
        </>
      )}
    </section>
  );
}

/** Search text and status and type filters narrowing the task list and the timeline. */
function TaskFilterBar({ filter, onChange }: { filter: TaskFilter; onChange: (filter: TaskFilter) => void }) {
  return (
    <div role="search" aria-label="Task filters" className="flex flex-1 flex-wrap items-center justify-end gap-2">
      <input
        type="search"
        aria-label="Search tasks"
        placeholder="Search title or description"
        className={cn(compactSelectClass, "w-56")}
        value={filter.query}
        onChange={(event) => onChange({ ...filter, query: event.target.value })}
      />
      <select
        aria-label="Status filter"
        className={compactSelectClass}
        value={filter.status ?? "all"}
        onChange={(event) =>
          onChange({ ...filter, status: event.target.value === "all" ? null : (event.target.value as ShownStatus) })
        }
      >
        <option value="all">All statuses</option>
        {shownStatuses.map((status) => (
          <option key={status} value={status}>
            {statusLabels[status]}
          </option>
        ))}
      </select>
      <select
        aria-label="Type filter"
        className={compactSelectClass}
        value={filter.type ?? "all"}
        onChange={(event) =>
          onChange({ ...filter, type: event.target.value === "all" ? null : (event.target.value as TaskType) })
        }
      >
        <option value="all">All types</option>
        {taskTypes.map((type) => (
          <option key={type} value={type}>
            {type}
          </option>
        ))}
      </select>
      {isFilterActive(filter) && (
        <Button size="xs" variant="ghost" onClick={() => onChange(emptyTaskFilter)}>
          <X />
          Clear filters
        </Button>
      )}
    </div>
  );
}

/** Statuses whose task is not being worked on, so it can be deleted with its worktree. */
const deletableStatuses: TaskStatus[] = ["planned", "done"];

type Confirmation = "delete" | "force-delete";

const confirmLabels: Record<Confirmation, string> = {
  delete: "Confirm delete",
  "force-delete": "Confirm force delete",
};

function TaskTimeline({ task, detailed }: { task: Task; detailed: boolean }) {
  const milestones = reachedMilestones(task.milestones);
  if (milestones.length === 0) {
    return null;
  }
  if (detailed) {
    return (
      <DetailField label="Timeline">
        <dl aria-label={`Timeline of #${task.id}`} className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
          {milestones.map(({ label, at }) => (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{label}</dt>
              <dd>
                <time dateTime={new Date(at * 1000).toISOString()}>{new Date(at * 1000).toLocaleString()}</time>
              </dd>
            </div>
          ))}
        </dl>
      </DetailField>
    );
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
  detailed = false,
}: {
  task: Task;
  /** Tasks of every workspace; any of them can become a prerequisite. */
  tasks: Task[];
  context: TaskContext;
  showWorkspace: boolean;
  /** Shown on its own, as in the timeline's details sheet: nothing is cut off and fields are labeled. */
  detailed?: boolean;
}) {
  const herdr = useHerdrStatus();
  const agentStatuses = useAgentStatuses();
  const pullRequestStatuses = usePullRequestStatuses();
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
  const closeWorkspace = useCloseTaskWorkspace();
  const [editing, setEditing] = useState(false);
  // The user chose to keep the herdr workspace of this paused task open for now.
  const [keepingWorkspace, setKeepingWorkspace] = useState(false);
  const [concluding, setConcluding] = useState(false);
  const [linkingIssue, setLinkingIssue] = useState(false);
  const [confirming, setConfirming] = useState<Confirmation | null>(null);
  // A prerequisite that would pause this started task, waiting for confirmation.
  const [confirmingPrerequisite, setConfirmingPrerequisite] = useState<TaskId | null>(null);
  const herdrRunning = herdr.data?.running === true;
  const inReview = task.status === "in_review";
  const active = task.status === "in_progress" || inReview;
  const hasWorktree = active && task.worktreePath !== null;
  const agentStatus = agentStatuses.data?.find((entry) => entry.taskId === task.id)?.status;
  const pullRequestStatus = inReview
    ? pullRequestStatuses.data?.find((entry) => entry.taskId === task.id)
    : undefined;
  const finishing = refreshCommits.isPending || openPullRequest.isPending || completeTask.isPending;
  const final = task.status === "done";
  const paused = isPaused(task);
  const startable = task.status === "planned" && dependenciesDone(task, context.byId);
  // Prerequisites concluded without merging: this task cannot build on them.
  const notAdopted = task.status === "planned" ? task.dependsOn.filter((id) => isNotAdopted(context.byId.get(id))) : [];
  // A paused task may still have the worktree it had while it was worked on.
  const removableWorktree = !active && task.worktreePath !== null;
  // A rejected delete means the worktree has local changes; offer to discard them.
  const dirtyWorktree =
    removableWorktree && deleteTask.error instanceof CommandFailure && deleteTask.error.kind === "rejected";
  const dependenciesEditable = dependencyEditableStatuses.includes(task.status);
  // A planned task can start (or a paused one resume) once these prerequisites are done.
  const waitingOn =
    task.status === "planned" && notAdopted.length === 0
      ? task.dependsOn.filter((id) => context.byId.get(id)?.status !== "done")
      : [];
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
    closeWorkspace.error ??
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
    <li className={cn("flex flex-col", detailed ? "gap-3 p-4" : "gap-1.5 px-4 py-3")}>
      {editing ? (
        <TaskEditForm task={task} onDone={() => setEditing(false)} />
      ) : (
        <div className={cn("flex min-w-0 flex-col", detailed ? "gap-1.5" : "gap-0.5")}>
          <div className={cn("flex min-w-0 items-center gap-2", detailed && "flex-wrap")}>
            <StatusBadge task={task} byId={context.byId} />
            <span className="shrink-0 text-xs tabular-nums text-muted-foreground">#{task.id}</span>
            {showWorkspace && (
              <span className="shrink-0 rounded-md bg-muted px-1.5 text-xs text-muted-foreground">
                {context.workspaceName(task.workspaceId)}
              </span>
            )}
            <TaskTypeBadge type={task.taskType} />
            {!detailed && (
              <span className="min-w-0 flex-1 truncate text-sm font-medium" title={task.title}>
                {task.title}
              </span>
            )}
            {pullRequestStatus && <PullRequestBadges taskId={task.id} status={pullRequestStatus} />}
            {active && agentStatus && <AgentBadge taskId={task.id} status={agentStatus} />}
          </div>
          {detailed && <h3 className="text-base font-semibold leading-snug break-words">{task.title}</h3>}
          {detailed && task.description.trim() && (
            <Suspense
              fallback={<p className="whitespace-pre-line break-words text-sm text-muted-foreground">{task.description}</p>}
            >
              <Markdown source={task.description} />
            </Suspense>
          )}
          {!detailed && markdownPreview(task.description) && (
            <p className="line-clamp-2 text-xs text-muted-foreground">{markdownPreview(task.description)}</p>
          )}
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
                notAdopted.includes(id) && "border-rose-300 text-rose-700 dark:border-rose-800 dark:text-rose-300",
              )}
              title={
                notAdopted.includes(id)
                  ? `${context.byId.get(id)?.title} (not adopted)`
                  : context.byId.get(id)?.title
              }
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
              title={
                task.status === "in_progress"
                  ? "A prerequisite that is not done pauses this task and closes its herdr workspace; the worktree is kept"
                  : undefined
              }
              className={cn(compactSelectClass, "max-w-48")}
              value=""
              disabled={changingDependencies}
              onChange={(e) => {
                if (e.target.value) {
                  const prerequisite = Number(e.target.value);
                  if (pausesStartedTask(task, context.byId.get(prerequisite))) {
                    setConfirmingPrerequisite(prerequisite);
                  } else {
                    addDependency.mutate({ id: task.id, dependsOn: prerequisite });
                  }
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
      {confirmingPrerequisite !== null && (
        <PrerequisiteConfirmation
          taskId={task.id}
          prerequisite={taskRef(confirmingPrerequisite, task.workspaceId, context)}
          pending={addDependency.isPending}
          onConfirm={() =>
            addDependency.mutate(
              { id: task.id, dependsOn: confirmingPrerequisite },
              { onSettled: () => setConfirmingPrerequisite(null) },
            )
          }
          onCancel={() => setConfirmingPrerequisite(null)}
        />
      )}
      {notAdopted.length > 0 && (
        <p
          role="note"
          aria-label={`Blocked prerequisites of #${task.id}`}
          className="text-xs text-rose-700 dark:text-rose-300"
        >
          {notAdopted.map((id) => taskRef(id, task.workspaceId, context)).join(", ")}{" "}
          {notAdopted.length === 1 ? "was" : "were"} not adopted. Remove the dependency or change the plan to
          start this task.
        </p>
      )}
      {waitingOn.length > 0 && (
        <p
          role="note"
          aria-label={`Waiting prerequisites of #${task.id}`}
          className={cn("text-xs", paused ? "text-amber-700 dark:text-amber-300" : "text-muted-foreground")}
        >
          Waiting on {waitingOn.map((id) => taskRef(id, task.workspaceId, context)).join(", ")}
        </p>
      )}
      {paused && waitingOn.length > 0 && task.herdrWorkspaceId !== null && (
        <OpenWorkspaceNote
          taskId={task.id}
          collapsed={keepingWorkspace}
          disabled={!herdrRunning || closeWorkspace.isPending}
          onClose={() => closeWorkspace.mutate(task.id)}
          onKeep={() => setKeepingWorkspace(true)}
        />
      )}
      {hasWorktree && <BranchFreshnessNote taskId={task.id} />}
      <TaskTimeline task={task} detailed={detailed} />
      {detailed && task.branch && (
        <DetailField label="Branch">
          <span className="break-all font-mono select-all">{task.branch}</span>
        </DetailField>
      )}
      {detailed && task.worktreePath && (
        <DetailField label="Worktree">
          <span className="break-all font-mono select-all">{task.worktreePath}</span>
        </DetailField>
      )}
      {detailed && task.pullRequestUrl && (
        <DetailField label="Pull request">
          <span className="break-all font-mono select-all">{task.pullRequestUrl}</span>
          {task.pullRequest && (
            <span aria-label={`Pull request of #${task.id}`} className="text-muted-foreground">
              {pullRequestStats(task.pullRequest)}
            </span>
          )}
        </DetailField>
      )}
      {!detailed && task.branch && (
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
      {!detailed && task.pullRequestUrl && (
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
      {(task.issue !== null || linkingIssue) && (
        <TaskIssue task={task} detailed={detailed} linking={linkingIssue} onDone={() => setLinkingIssue(false)} />
      )}
      <CommitList task={task} detailed={detailed} />
      <TaskNotes task={task} detailed={detailed} />
      {final && !editing && <CompletionNote task={task} />}
      {actionError && (
        <p role="alert" className="text-xs text-destructive">
          {actionError.message}
        </p>
      )}
      {concluding && !editing && <ConcludeForm task={task} onDone={() => setConcluding(false)} />}
      {!editing && !concluding && (
        <div className="flex flex-wrap items-center gap-1">
          {task.status === "planned" && (
            <Button
              size="xs"
              aria-label={`${paused ? "Resume" : "Start"} #${task.id}`}
              title={
                startable
                  ? undefined
                  : notAdopted.length > 0
                    ? "A dependency was not adopted; remove it to start"
                    : "Waits until all its dependencies are done"
              }
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
          {active && (
            <Button
              size="xs"
              variant="outline"
              aria-label={`Conclude #${task.id} without merging`}
              title="Finish without merging, e.g. research whose result is not adopted; an open pull request is closed"
              disabled={finishing || (task.herdrWorkspaceId !== null && !herdrRunning)}
              onClick={() => setConcluding(true)}
            >
              <CircleSlash />
              Conclude
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
                {!final && task.issue === null && !linkingIssue && (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    aria-label={`Link issue to #${task.id}`}
                    title="Link a GitHub issue; the pull request closes it on merge"
                    onClick={() => setLinkingIssue(true)}
                  >
                    <CircleDot />
                  </Button>
                )}
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
/** Warns when a started task's branch is behind its base, e.g. after a prerequisite was merged. */
function BranchFreshnessNote({ taskId }: { taskId: TaskId }) {
  const freshness = useBranchFreshness(taskId, true);
  const label = `Base freshness of #${taskId}`;
  if (freshness.error) {
    return (
      <p role="note" aria-label={label} className="text-xs text-muted-foreground" title={freshness.error.message}>
        Could not check whether the branch is behind its base
      </p>
    );
  }
  const data = freshness.data;
  const conflicting = data?.mergeable === "conflicting";
  if (!data || (data.behind === 0 && !conflicting)) {
    return null;
  }
  const parts: string[] = [];
  if (data.behind > 0) {
    parts.push(`${data.behind} ${data.behind === 1 ? "commit" : "commits"} behind ${data.base}`);
  }
  if (conflicting) {
    parts.push("pull request has merge conflicts");
  } else if (data.mergeable === "mergeable") {
    parts.push("no merge conflicts");
  }
  return (
    <p
      role="note"
      aria-label={label}
      data-conflicting={conflicting || undefined}
      className={cn(
        "text-xs",
        conflicting ? "text-rose-700 dark:text-rose-300" : "text-amber-700 dark:text-amber-300",
      )}
      title={
        [data.fetchError && `Fetching origin failed: ${data.fetchError}`, data.mergeableError]
          .filter(Boolean)
          .join("\n") || undefined
      }
    >
      {parts.join("; ")}
      {data.fetchError && " (not up to date: fetching origin failed)"}
    </p>
  );
}

function StatusBadge({ task, byId }: { task: Task; byId: ReadonlyMap<TaskId, Task> }) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium",
        statusStyle(task, byId),
      )}
    >
      <span aria-hidden className="size-1.5 rounded-full bg-current opacity-80" />
      {statusLabel(task, byId)}
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
      <MarkdownEditor
        label={`Description of #${task.id}`}
        placeholder="Description (optional, Markdown)"
        value={description}
        onChange={setDescription}
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

/**
 * Concludes an active task without merging: the conclusion, why the work is not adopted, is
 * required and becomes its completion note.
 */
function ConcludeForm({ task, onDone }: { task: Task; onDone: () => void }) {
  const concludeTask = useConcludeTask();
  const [note, setNote] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    concludeTask.mutate({ id: task.id, note: note.trim() }, { onSuccess: onDone });
  }

  return (
    <form onSubmit={submit} aria-label={`Conclude #${task.id}`} className="flex flex-col gap-1">
      <p className="text-xs text-muted-foreground">
        Finish #{task.id} without merging. It is marked not adopted, its pull request is closed with the
        conclusion as a comment, and the branch is kept. Tasks that depend on it stay blocked.
      </p>
      <textarea
        className={cn(inputClass, "h-auto min-h-16 py-1.5")}
        aria-label={`Conclusion of #${task.id}`}
        placeholder="What was tried, what was found, and why it is not adopted."
        value={note}
        onChange={(e) => setNote(e.target.value)}
        required
        autoFocus
      />
      <div className="flex gap-1">
        <Button type="submit" size="xs" disabled={concludeTask.isPending || note.trim() === ""}>
          <CircleSlash />
          {concludeTask.isPending ? "Concluding..." : "Conclude without merge"}
        </Button>
        <Button type="button" size="xs" variant="ghost" onClick={onDone}>
          Back
        </Button>
      </div>
      {concludeTask.isError && (
        <p role="alert" className="text-xs text-destructive">
          {concludeTask.error.message}
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
        <Button
          type="submit"
          size="xs"
          // The conclusion of work that was not adopted cannot be cleared.
          disabled={setNote.isPending || note.trim() === saved || (isNotAdopted(task) && note.trim() === "")}
        >
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

/** An issue number typed by the user, with or without `#`; `null` unless it is a positive integer. */
function parseIssueNumber(raw: string): number | null {
  const match = /^#?(\d{1,9})$/.exec(raw.trim());
  const number = match ? Number(match[1]) : 0;
  return number > 0 ? number : null;
}

/**
 * The GitHub issue of a task, which its pull request closes on merge, or the form that
 * links an existing issue or creates one from the task.
 */
function TaskIssue({
  task,
  detailed,
  linking,
  onDone,
}: {
  task: Task;
  detailed: boolean;
  linking: boolean;
  onDone: () => void;
}) {
  const linkIssue = useLinkIssue();
  const createIssue = useCreateIssue();
  const [number, setNumber] = useState("");
  const parsed = parseIssueNumber(number);
  const pending = linkIssue.isPending || createIssue.isPending;
  const error = linkIssue.error ?? createIssue.error;
  const editable = task.status !== "done";

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (parsed !== null) {
      linkIssue.mutate({ id: task.id, number: parsed }, { onSuccess: onDone });
    }
  }

  const issue = task.issue;
  const content = issue ? (
    <p className="flex min-w-0 items-center gap-1 text-xs">
      {!detailed && <CircleDot aria-hidden className="size-3 shrink-0 text-muted-foreground" />}
      {!detailed && <span className="sr-only">Issue </span>}
      <span aria-label={`Issue of #${task.id}`} title={issue.url} className="shrink-0 font-medium tabular-nums">
        #{issue.number}
      </span>
      <span className={cn("min-w-0 font-mono text-muted-foreground select-all", detailed ? "break-all" : "truncate")}>
        {issue.url}
      </span>
      {editable && (
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={`Unlink issue from #${task.id}`}
          title="Unlink the issue; a pull request keeps its closing reference"
          disabled={pending}
          onClick={() => linkIssue.mutate({ id: task.id, number: null })}
        >
          <X />
        </Button>
      )}
    </p>
  ) : (
    linking && (
      <form onSubmit={submit} aria-label={`Issue form of #${task.id}`} className="flex flex-wrap items-center gap-1">
        <input
          className={cn(inputClass, "h-7 w-28")}
          inputMode="numeric"
          placeholder="Issue #"
          aria-label={`Issue number of #${task.id}`}
          value={number}
          onChange={(e) => setNumber(e.target.value)}
          autoFocus
        />
        <Button type="submit" size="xs" disabled={parsed === null || pending}>
          {linkIssue.isPending ? "Linking..." : "Link"}
        </Button>
        <Button
          type="button"
          size="xs"
          variant="outline"
          aria-label={`Create issue for #${task.id}`}
          title="Open a GitHub issue with the title and description of this task and link it"
          disabled={pending}
          onClick={() => createIssue.mutate(task.id, { onSuccess: onDone })}
        >
          <Plus />
          {createIssue.isPending ? "Creating..." : "Create issue"}
        </Button>
        <Button type="button" size="xs" variant="ghost" onClick={onDone}>
          Discard
        </Button>
      </form>
    )
  );
  return (
    <>
      {detailed && issue ? <DetailField label="Issue">{content}</DetailField> : content}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error.message}
        </p>
      )}
    </>
  );
}

/**
 * Asks whether to close the herdr workspace that a task paused on a new prerequisite still
 * has, e.g. because an agent added the prerequisite through MCP. Collapsed after "Keep
 * open", it still offers to close it.
 */
function OpenWorkspaceNote({
  taskId,
  collapsed,
  disabled,
  onClose,
  onKeep,
}: {
  taskId: TaskId;
  collapsed: boolean;
  disabled: boolean;
  onClose: () => void;
  onKeep: () => void;
}) {
  const close = (
    <Button
      size="xs"
      variant="outline"
      aria-label={`Close herdr workspace of #${taskId}`}
      title="Close the herdr workspace, which ends its agent session; the worktree is kept"
      disabled={disabled}
      onClick={onClose}
    >
      <X />
      Close workspace
    </Button>
  );
  if (collapsed) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span>herdr workspace still open</span>
        {close}
      </div>
    );
  }
  return (
    <div
      role="group"
      aria-label={`Herdr workspace of paused #${taskId}`}
      className="flex flex-col gap-2 rounded-md border border-amber-300 bg-amber-50 p-2 text-xs text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      <p>
        Paused for a new prerequisite, but its herdr workspace is still open. Close it? This ends its agent
        session; the worktree is kept for resuming.
      </p>
      <div className="flex gap-2">
        {close}
        <Button size="xs" variant="ghost" aria-label={`Keep herdr workspace of #${taskId} open`} onClick={onKeep}>
          Keep open
        </Button>
      </div>
    </div>
  );
}

/** A labeled block of the task details sheet. */
function DetailField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section aria-label={label} className="flex flex-col gap-0.5 text-xs">
      <h4 className="font-medium text-muted-foreground">{label}</h4>
      {children}
    </section>
  );
}

function CommitList({ task, detailed }: { task: Task; detailed: boolean }) {
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
  const list = (
    <ul aria-label={`Commits of #${task.id}`} className={cn("flex flex-col gap-0.5", !detailed && "mt-1")}>
      {commits.data.map((commit) => (
        <li key={commit.sha} className="flex gap-2 text-xs" title={commit.author}>
          <code className="shrink-0 text-muted-foreground">{commit.sha.slice(0, 7)}</code>
          <span className={detailed ? "min-w-0 break-words" : "truncate"}>{commit.message.split("\n", 1)[0]}</span>
        </li>
      ))}
    </ul>
  );
  return detailed ? <DetailField label={`Commits (${commits.data.length})`}>{list}</DetailField> : list;
}

const noteKindLabels: Record<NoteKind, string> = { progress: "Progress", blocker: "Blocker" };

function NoteKindBadge({ kind }: { kind: NoteKind }) {
  return (
    <span
      data-note-kind={kind}
      className={cn(
        "shrink-0 rounded-md px-1.5 font-medium",
        kind === "blocker"
          ? "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300"
          : "bg-muted text-muted-foreground",
      )}
    >
      {noteKindLabels[kind]}
    </span>
  );
}

/**
 * Notes agents left on the task through the MCP server: all of them in the details sheet,
 * the latest one in a list row.
 */
function TaskNotes({ task, detailed }: { task: Task; detailed: boolean }) {
  const notes = task.notes;
  if (notes.length === 0) {
    return null;
  }
  if (!detailed) {
    const latest = notes[notes.length - 1];
    return (
      <p
        aria-label={`Latest note of #${task.id}`}
        className="flex min-w-0 items-center gap-1.5 text-xs"
        title={new Date(latest.at * 1000).toLocaleString()}
      >
        <NoteKindBadge kind={latest.kind} />
        <span className="min-w-0 truncate text-muted-foreground">{markdownPreview(latest.body)}</span>
        {notes.length > 1 && (
          <span className="shrink-0 text-muted-foreground">{`+${notes.length - 1} more`}</span>
        )}
      </p>
    );
  }
  return (
    <DetailField label={`Notes (${notes.length})`}>
      <ol aria-label={`Notes of #${task.id}`} className="flex flex-col gap-2">
        {notes.map((note, index) => (
          <li key={index} className="flex flex-col gap-0.5">
            <div className="flex items-center gap-1.5">
              <NoteKindBadge kind={note.kind} />
              <time className="text-muted-foreground" dateTime={new Date(note.at * 1000).toISOString()}>
                {new Date(note.at * 1000).toLocaleString()}
              </time>
            </div>
            <Suspense fallback={<p className="whitespace-pre-line break-words">{note.body}</p>}>
              <Markdown source={note.body} />
            </Suspense>
          </li>
        ))}
      </ol>
    </DetailField>
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

/** Picker value of the "New branch..." option; `:` cannot appear in a branch name. */
const NEW_BRANCH = ":new";

/**
 * The location and default base branch of the workspace; the branch that new task branches
 * start from and pull requests target can be changed in place, either to a branch of the
 * repository or to a new branch created from one.
 */
function BaseBranchForm({ workspace }: { workspace: Workspace }) {
  const updateWorkspace = useUpdateWorkspace();
  const createBaseBranch = useCreateBaseBranch();
  const tasks = useTasks();
  // An open pull request targets the current branch, so it stays while a task is in review.
  const inReview = tasks.data?.filter((task) => task.workspaceId === workspace.id && task.status === "in_review");
  const locked = inReview === undefined || inReview.length > 0;
  const [draft, setDraft] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [newFrom, setNewFrom] = useState(workspace.defaultBaseBranch);
  const branches = useWorkspaceBranches(workspace.id, draft !== null);
  const location = describeLocation(workspace.location);
  const current = workspace.defaultBaseBranch;
  const names = [...new Set([...(branches.data?.names ?? []), current])].sort();
  const name = newName.trim();
  const pending = updateWorkspace.isPending || createBaseBranch.isPending;
  const canSave =
    !pending && (draft === NEW_BRANCH ? name !== "" && !names.includes(name) && newFrom !== "" : draft !== current);
  const error = updateWorkspace.error ?? createBaseBranch.error;

  function open() {
    updateWorkspace.reset();
    createBaseBranch.reset();
    setNewName("");
    setNewFrom(current);
    setDraft(current);
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (draft === null || !canSave) return;
    const close = { onSuccess: () => setDraft(null) };
    if (draft === NEW_BRANCH) {
      createBaseBranch.mutate({ workspace: workspace.id, name, from: newFrom }, close);
    } else {
      updateWorkspace.mutate({ ...workspace, defaultBaseBranch: draft }, close);
    }
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
          onClick={open}
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
        <select
          id="base-branch"
          className={cn(compactSelectClass, "max-w-56 font-mono")}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          autoFocus
        >
          {names.map((branch) => (
            <option key={branch} value={branch}>
              {branch}
            </option>
          ))}
          <option value={NEW_BRANCH}>New branch...</option>
        </select>
        {branches.isFetching && <span className="text-xs">Loading branches...</span>}
      </div>
      {draft === NEW_BRANCH && (
        <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm text-muted-foreground">
          <label htmlFor="new-base-branch" className="shrink-0">
            Name
          </label>
          <input
            id="new-base-branch"
            aria-label="New branch name"
            className={cn(inputClass, "max-w-56 font-mono")}
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="release/2026"
            autoFocus
          />
          <label htmlFor="new-base-branch-from" className="shrink-0">
            from
          </label>
          <select
            id="new-base-branch-from"
            aria-label="Start new branch from"
            className={cn(compactSelectClass, "max-w-56 font-mono")}
            value={newFrom}
            onChange={(e) => setNewFrom(e.target.value)}
          >
            {names.map((branch) => (
              <option key={branch} value={branch}>
                {branch}
              </option>
            ))}
          </select>
          {names.includes(name) && <span className="text-xs">Branch {name} already exists; pick it above.</span>}
        </div>
      )}
      <div className="flex items-center gap-2">
        <Button type="submit" size="sm" variant="outline" disabled={!canSave}>
          {draft === NEW_BRANCH ? "Create and save" : "Save"}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setDraft(null)}>
          Discard
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        New task branches start from this branch and pull requests target it.
        {draft === NEW_BRANCH && " The new branch is pushed to origin."}
      </p>
      {branches.error && (
        <p role="alert" className="text-xs text-destructive">
          Failed to load branches: {branches.error.message}
        </p>
      )}
      {branches.data?.fetchError && (
        <p className="text-xs text-muted-foreground" title={branches.data.fetchError}>
          Could not fetch origin; showing the branches known locally.
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error.message}
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

export type TaskDraft = { taskType: TaskType; title: string; description: string; issue: string };

const emptyDraft: TaskDraft = { taskType: "feat", title: "", description: "", issue: "" };

function NewTaskForm({
  workspace,
  draft,
  onChange,
}: {
  workspace: Workspace;
  draft: TaskDraft;
  onChange: (draft: TaskDraft) => void;
}) {
  const createTask = useCreateTask();
  const { taskType, title, description, issue } = draft;
  const issueNumber = parseIssueNumber(issue);
  const invalidIssue = issue.trim() !== "" && issueNumber === null;
  const setTaskType = (value: TaskType) => onChange({ ...draft, taskType: value });
  const setTitle = (value: string) => onChange({ ...draft, title: value });
  const setDescription = (value: string) => onChange({ ...draft, description: value });
  const setIssue = (value: string) => onChange({ ...draft, issue: value });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const added = () => onChange({ ...draft, title: "", description: "", issue: "" });
    createTask.mutate(
      { workspaceId: workspace.id, taskType, title, description, issue: issueNumber },
      {
        onSuccess: added,
        // The task exists even though its issue was not linked; adding it again would duplicate it.
        onError: (error) => error instanceof IssueLinkFailure && added(),
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
        <Button type="submit" disabled={createTask.isPending || invalidIssue}>
          <Plus />
          Add task
        </Button>
      </div>
      <div className="flex items-start gap-2">
        <MarkdownEditor
          label="Description"
          placeholder="Description (optional, Markdown)"
          value={description}
          onChange={setDescription}
          className="flex-1"
        />
        <input
          className={cn(inputClass, "mt-7 w-36 shrink-0")}
          inputMode="numeric"
          placeholder="Issue # (optional)"
          aria-label="Issue number"
          aria-invalid={invalidIssue}
          title="An issue of the workspace repository; the pull request closes it on merge"
          value={issue}
          onChange={(e) => setIssue(e.target.value)}
        />
      </div>
      {createTask.isError && (
        <p role="alert" className="text-xs text-destructive">
          {createTask.error.message}
        </p>
      )}
    </form>
  );
}
