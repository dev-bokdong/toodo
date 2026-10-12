import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { inputClass, describeLocation } from "@/components/workspace-list";
import type { Task, TaskStatus, TaskType, Workspace } from "@/lib/bindings";
import {
  machineOf,
  useCreateTask,
  useHerdrStatus,
  useMachineSettings,
  useOpenTaskInHerdr,
  useSaveMachineSettings,
  useStartTask,
  useTasks,
} from "@/lib/queries";
import { cn } from "@/lib/utils";

const taskTypes: TaskType[] = ["feat", "fix", "modify", "refactor", "docs", "test", "chore"];

const statusLabels: Record<TaskStatus, string> = {
  planned: "Planned",
  ready: "Ready",
  in_progress: "In progress",
  done: "Done",
  cancelled: "Cancelled",
};

const statusStyles: Record<TaskStatus, string> = {
  planned: "bg-muted text-muted-foreground",
  ready: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-200",
  in_progress: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-200",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-200",
  cancelled: "bg-muted text-muted-foreground line-through",
};

export function TaskList({ workspace }: { workspace: Workspace }) {
  const tasks = useTasks(workspace.id);

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
        <ul aria-label="Tasks" className="flex flex-col divide-y rounded-lg border">
          {tasks.data.map((task) => (
            <TaskRow key={task.id} task={task} />
          ))}
        </ul>
      )}
    </section>
  );
}

function TaskRow({ task }: { task: Task }) {
  const herdr = useHerdrStatus();
  const startTask = useStartTask();
  const openTask = useOpenTaskInHerdr();
  const herdrRunning = herdr.data?.running === true;
  const actionError = startTask.error ?? openTask.error;

  return (
    <li className="flex items-start gap-3 px-3 py-2">
      <span className="mt-0.5 w-8 shrink-0 text-xs tabular-nums text-muted-foreground">#{task.id}</span>
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex items-center gap-2">
          <span className="rounded-md border px-1.5 text-xs font-medium">{task.taskType}</span>
          <span className="truncate text-sm font-medium">{task.title}</span>
        </div>
        {task.description && (
          <p className="line-clamp-2 text-xs text-muted-foreground">{task.description}</p>
        )}
        {task.dependsOn.length > 0 && (
          <p className="text-xs text-muted-foreground">
            Depends on {task.dependsOn.map((id) => `#${id}`).join(", ")}
          </p>
        )}
        {task.branch && (
          <p className="truncate font-mono text-xs text-muted-foreground" title={task.worktreePath ?? undefined}>
            {task.branch}
            {task.worktreePath && ` \u00b7 ${task.worktreePath}`}
          </p>
        )}
        {actionError && (
          <p role="alert" className="text-xs text-destructive">
            {actionError.message}
          </p>
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
      {task.status === "in_progress" && task.herdrWorkspaceId && (
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
      <span className={cn("shrink-0 rounded-full px-2 py-0.5 text-xs font-medium", statusStyles[task.status])}>
        {statusLabels[task.status]}
      </span>
    </li>
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
