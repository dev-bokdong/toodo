import { useEffect, useRef, useState, type ReactNode } from "react";
import { XIcon } from "lucide-react";
import { compactSelectClass } from "@/components/workspace-list";
import {
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  useStore,
  useViewport,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { Button } from "@/components/ui/button";
import type { Task, TaskId, Timestamp } from "@/lib/bindings";
import { useAddDependency, useRemoveDependency } from "@/lib/queries";
import {
  canAddDependency,
  dependencyEdges,
  dependencyEditableStatuses,
  isPaused,
  layoutTimeline,
  relatedTasks,
  timelineAxis,
  timelineScales,
  timelineYears,
  yearPeriod,
  type TimelineBar,
  type TimelineLayout,
  type TimelinePhase,
  type TimelineScale,
} from "@/lib/task-graph";
import { useThemePreference } from "@/lib/theme";
import { formatDate, reachedMilestones } from "@/lib/timeline";
import { cn } from "@/lib/utils";

type BarNode = Node<
  { bar: TimelineBar; statusLabel: string; statusStyle: string; workspace: string | null; dimmed: boolean },
  "bar"
>;

const nodeTypes = { bar: TimelineBarView };

/** Height of the axis header: a row of coarse ticks above a row of fine ticks. */
const axisRowHeight = 18;
const axisHeight = 2 * axisRowHeight;
const rowsTop = axisHeight + 8;
const rowHeight = 40;
const barHeight = 30;
/** How often the current-time cursor, and the bars attached to it, move. */
const nowRefreshMs = 30_000;
/** Ticks this far outside the visible area are still drawn, so labels never pop in at the edge. */
const tickOverscan = 200;

const scaleLabels: Record<TimelineScale, string> = { hour: "Hour", day: "Day", week: "Week", month: "Month" };

const phaseStyles: Record<TimelinePhase, string> = {
  in_progress: "bg-blue-500",
  in_review: "bg-violet-500",
  paused: "bg-muted-foreground/30",
};

const phaseLabels: Record<TimelinePhase, string> = {
  in_progress: "In progress",
  in_review: "In review",
  paused: "Paused",
};

const pad = (value: number) => String(value).padStart(2, "0");

/** The current time in Unix seconds, refreshed every `nowRefreshMs`. */
function useNow(): Timestamp {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), nowRefreshMs);
    return () => clearInterval(timer);
  }, []);
  return now;
}

export function TaskGraph({
  tasks,
  allTasks,
  dimmed,
  workspaceLabel,
  statusLabel,
  statusStyle,
  renderDetail,
}: {
  /** Tasks drawn on the timeline. */
  tasks: Task[];
  /** Tasks of every workspace, which new dependencies are checked against. */
  allTasks: Task[];
  /** Tasks drawn only for context, such as prerequisites outside the shown workspace. */
  dimmed: ReadonlySet<TaskId>;
  /** Workspace name shown on a bar, or null to leave it out. */
  workspaceLabel: (task: Task) => string | null;
  statusLabel: (task: Task) => string;
  statusStyle: (task: Task) => string;
  /** Details of the task selected on the timeline, with its actions. */
  renderDetail: (task: Task) => ReactNode;
}) {
  const addDependency = useAddDependency();
  const removeDependency = useRemoveDependency();
  const [selectedEdgeIds, setSelectedEdgeIds] = useState<ReadonlySet<string>>(new Set());
  const [selectedTaskId, setSelectedTaskId] = useState<TaskId | null>(null);
  const [scale, setScale] = useState<TimelineScale>("week");
  const [year, setYear] = useState<number | null>(null);
  const colorMode = useThemePreference();
  const container = useRef<HTMLDivElement>(null);
  const now = useNow();
  const layout = layoutTimeline(tasks, { now, scale, period: year === null ? null : yearPeriod(year) });
  const placed = new Set(layout.bars.map((bar) => bar.task.id));
  const dependencies = dependencyEdges(tasks).filter((edge) => placed.has(edge.source) && placed.has(edge.target));
  // A task that left the timeline, through a filter or a deletion, is no longer selected.
  const selectedTask = placed.has(selectedTaskId ?? -1) ? tasks.find((task) => task.id === selectedTaskId) : undefined;
  // While a task is selected, everything outside its dependency chain fades.
  const chain = selectedTask === undefined ? null : relatedTasks(tasks, selectedTask.id);
  const inChain = (id: TaskId) => chain === null || chain.has(id);
  const changing = addDependency.isPending || removeDependency.isPending;
  const editable = (target: number) =>
    dependencyEditableStatuses.includes(tasks.find((task) => task.id === target)?.status ?? "done");
  const height = rowsTop + layout.rows * rowHeight;
  const panelHeight = Math.min(Math.max(height + 32, 160), 448);
  // Rows can be panned vertically only as far as they go; never into empty space.
  const translateExtent: [[number, number], [number, number]] = [
    [-Infinity, 0],
    [Infinity, Math.max(height + 8, panelHeight)],
  ];

  const nodes: BarNode[] = layout.bars.map((bar) => ({
    id: String(bar.task.id),
    type: "bar",
    position: { x: bar.x, y: rowsTop + bar.row * rowHeight + (rowHeight - barHeight) / 2 },
    data: {
      bar,
      statusLabel: statusLabel(bar.task),
      statusStyle: statusStyle(bar.task),
      workspace: workspaceLabel(bar.task),
      dimmed: dimmed.has(bar.task.id) || !inChain(bar.task.id),
    },
    selected: bar.task.id === selectedTask?.id,
    deletable: false,
    width: bar.width,
    height: barHeight,
  }));
  const edges: Edge[] = dependencies.map((edge) => ({
    id: edge.id,
    source: String(edge.source),
    target: String(edge.target),
    selected: selectedEdgeIds.has(edge.id),
    deletable: editable(edge.target),
    markerEnd: { type: MarkerType.ArrowClosed },
    style: inChain(edge.source) && inChain(edge.target) ? undefined : { opacity: 0.2 },
  }));
  const selected = dependencies.filter((edge) => selectedEdgeIds.has(edge.id) && editable(edge.target));

  function showLatest(instance: ReactFlowInstance<BarNode>) {
    const visible = container.current?.clientWidth ?? 0;
    void instance.setViewport({ x: Math.min(16, visible - layout.width - 16), y: 0, zoom: 1 });
  }

  function connect({ source, target }: Connection) {
    addDependency.mutate({ id: Number(target), dependsOn: Number(source) });
  }

  function removeEdges(removed: { source: string; target: string }[]) {
    for (const edge of removed) {
      removeDependency.mutate({ id: Number(edge.target), dependsOn: Number(edge.source) });
    }
    setSelectedEdgeIds(new Set());
  }

  function changeNodes(changes: NodeChange<BarNode>[]) {
    const picked = changes.filter((change) => change.type === "select" && change.selected).pop();
    if (picked !== undefined && picked.type === "select") {
      setSelectedTaskId(Number(picked.id));
      setSelectedEdgeIds(new Set());
    } else if (
      changes.some((change) => change.type === "select" && !change.selected && Number(change.id) === selectedTaskId)
    ) {
      setSelectedTaskId(null);
    }
  }

  function changeEdges(changes: EdgeChange[]) {
    if (changes.some((change) => change.type === "select" && change.selected)) {
      setSelectedTaskId(null);
    }
    setSelectedEdgeIds((current) => {
      const next = new Set(current);
      for (const change of changes) {
        if (change.type === "select") {
          if (change.selected) {
            next.add(change.id);
          } else {
            next.delete(change.id);
          }
        }
      }
      return next;
    });
  }

  return (
    <section aria-label="Task timeline" className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <div role="group" aria-label="Timeline scale" className="flex gap-1">
          {timelineScales.map((option) => (
            <Button
              key={option}
              size="xs"
              variant={scale === option ? "secondary" : "ghost"}
              aria-pressed={scale === option}
              onClick={() => setScale(option)}
            >
              {scaleLabels[option]}
            </Button>
          ))}
        </div>
        <select
          aria-label="Timeline period"
          className={compactSelectClass}
          value={year ?? "all"}
          onChange={(event) => setYear(event.target.value === "all" ? null : Number(event.target.value))}
        >
          <option value="all">All time</option>
          {timelineYears(tasks, now).map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
        <ul aria-label="Phases" className="flex gap-2 text-muted-foreground">
          <li className="flex items-center gap-1">
            <span className="size-2 rotate-45 border border-muted-foreground" />
            Planned
          </li>
          <li className="flex items-center gap-1">
            <span className="h-2 border-l-2 border-emerald-500" />
            Ready
          </li>
          <li className="flex items-center gap-1">
            <span className="w-3 border-t border-dashed border-muted-foreground" />
            Waiting
          </li>
          <li className="flex items-center gap-1">
            <span className="h-2 w-3 rounded-sm border border-dashed border-muted-foreground" />
            Not started
          </li>
          {(Object.keys(phaseLabels) as TimelinePhase[]).map((phase) => (
            <li key={phase} className="flex items-center gap-1">
              <span className={cn("h-1.5 w-3 rounded-full", phaseStyles[phase])} />
              {phaseLabels[phase]}
            </li>
          ))}
        </ul>
      </div>
      <div className="flex min-h-7 flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>
          Tasks not started yet wait at the current time, after their prerequisites. Drag from a task's right handle
          to another task to make it a prerequisite.
        </span>
        {selected.length > 0 && (
          <Button
            size="xs"
            variant="outline"
            disabled={changing}
            onClick={() => removeEdges(selected.map((edge) => ({ source: String(edge.source), target: String(edge.target) })))}
          >
            Remove {selected.map((edge) => `#${edge.source} \u2192 #${edge.target}`).join(", ")}
          </Button>
        )}
      </div>
      {(addDependency.error ?? removeDependency.error) && (
        <p role="alert" className="text-xs text-destructive">
          {(addDependency.error ?? removeDependency.error)?.message}
        </p>
      )}
      <div
        className="flex gap-2"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            setSelectedTaskId(null);
          }
        }}
      >
        <div
          ref={container}
          className="min-w-0 flex-1 rounded-lg border"
          style={{ height: panelHeight }}
        >
          {layout.bars.length === 0 ? (
            <p className="p-3 text-sm text-muted-foreground">
              {year === null ? "No planned tasks yet." : `No tasks in ${year}.`}
            </p>
          ) : (
            <ReactFlow
              key={`${scale}-${year ?? "all"}`}
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onInit={showLatest}
              minZoom={1}
              maxZoom={1}
              zoomOnScroll={false}
              zoomOnPinch={false}
              zoomOnDoubleClick={false}
              panOnScroll
              translateExtent={translateExtent}
              nodesDraggable={false}
              nodesConnectable={!changing}
              isValidConnection={(c) => canAddDependency(allTasks, Number(c.source), Number(c.target))}
              onConnect={connect}
              onNodesChange={changeNodes}
              onPaneClick={() => setSelectedTaskId(null)}
              onEdgesChange={changeEdges}
              onEdgesDelete={removeEdges}
              deleteKeyCode={["Backspace", "Delete"]}
              colorMode={colorMode}
              proOptions={{ hideAttribution: true }}
            >
              <TimelineGrid layout={layout} />
              <TimelineHeader layout={layout} now={now} />
            </ReactFlow>
          )}
        </div>
        {(selectedTask !== undefined || layout.unplaced.length > 0) && (
          <div className={cn("flex shrink-0 flex-col gap-3", selectedTask === undefined ? "w-56" : "w-96")}>
            {selectedTask !== undefined && (
              <aside aria-label="Task details" className="flex flex-col gap-1 text-xs">
                <div className="flex items-center justify-between">
                  <h3 className="font-medium text-muted-foreground">Task details</h3>
                  <Button size="icon-xs" variant="ghost" aria-label="Close details" onClick={() => setSelectedTaskId(null)}>
                    <XIcon />
                  </Button>
                </div>
                <ul className="rounded-xl border bg-card shadow-xs">{renderDetail(selectedTask)}</ul>
              </aside>
            )}
            {layout.unplaced.length > 0 && (
              <aside aria-label="Tasks without a planned date" className="flex flex-col gap-1 text-xs">
                <h3 className="font-medium text-muted-foreground">Not on the timeline</h3>
                <ul className="flex flex-col gap-1">
                  {layout.unplaced.map((task) => (
                    <li key={task.id} className="flex items-center gap-1.5 rounded-md border px-2 py-1">
                      <span className="tabular-nums text-muted-foreground">#{task.id}</span>
                      <span className="truncate" title={task.title}>
                        {task.title}
                      </span>
                      <span className={cn("ml-auto shrink-0 rounded-full px-1.5", statusStyle(task))}>
                        {statusLabel(task)}
                      </span>
                    </li>
                  ))}
                </ul>
              </aside>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

/** Left and right ends, in flow coordinates, of the part of the timeline that is on screen. */
function useVisibleRange(): { from: number; to: number } {
  const { x } = useViewport();
  const width = useStore((state) => state.width);
  return { from: -x - tickOverscan, to: -x + width + tickOverscan };
}

/**
 * Vertical grid lines at each fine tick and the current-time cursor, behind the bars. Like
 * the header, they follow horizontal panning only, so they always run the panel's full height.
 */
function TimelineGrid({ layout }: { layout: TimelineLayout }) {
  const { x } = useViewport();
  const { from, to } = useVisibleRange();
  const { ticks } = timelineAxis(layout, from, to);
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden" style={{ zIndex: 0 }}>
      <div className="absolute inset-y-0 left-0" style={{ transform: `translateX(${x}px)` }}>
        {ticks.map((tick) => (
          <div key={tick.at} className="absolute inset-y-0 border-l border-border/60" style={{ left: tick.x }} />
        ))}
        {layout.nowX !== null && (
          <div className="absolute inset-y-0 border-l-2 border-red-500" style={{ left: layout.nowX }} />
        )}
      </div>
    </div>
  );
}

/**
 * The time axis above the rows, like a Gantt chart header: it follows horizontal panning
 * but stays at the top when the rows scroll.
 */
function TimelineHeader({ layout, now }: { layout: TimelineLayout; now: Timestamp }) {
  const { x } = useViewport();
  const { from, to } = useVisibleRange();
  const { ticks, majorTicks } = timelineAxis(layout, from, to);
  const date = new Date(now * 1000);
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-x-0 top-0 overflow-hidden border-b bg-background text-[10px] text-muted-foreground"
      style={{ height: axisHeight, zIndex: 5 }}
    >
      <div className="absolute left-0 top-0 h-full" style={{ transform: `translateX(${x}px)` }}>
        {majorTicks.map((tick) => (
          <div
            key={tick.at}
            className="absolute top-0 border-l border-border font-medium text-foreground"
            style={{ left: tick.x, width: tick.right - tick.x, height: axisRowHeight }}
          >
            {/* Keep the label of a period that starts off screen at the left edge, while it fits. */}
            <span
              className="block whitespace-nowrap pl-1"
              style={{ marginLeft: Math.max(0, Math.min(-x - tick.x, tick.right - tick.x - 80)) }}
            >
              {tick.label}
            </span>
          </div>
        ))}
        {ticks.map((tick) => (
          <div
            key={tick.at}
            className="absolute whitespace-nowrap border-l border-border pl-1"
            style={{ left: tick.x, top: axisRowHeight, height: axisRowHeight }}
          >
            {tick.label}
          </div>
        ))}
        {layout.nowX !== null && (
          <div className="absolute top-0 h-full border-l-2 border-red-500" style={{ left: layout.nowX }}>
            {/* Only the label row gets a background, so the fine ticks below stay visible. */}
            <span
              className="block whitespace-nowrap bg-background pl-1 font-medium text-red-500"
              style={{ height: axisRowHeight }}
            >
              Now {pad(date.getHours())}:{pad(date.getMinutes())}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}

function TimelineBarView({
  data: { bar, statusLabel, statusStyle, workspace, dimmed },
  selected,
}: NodeProps<BarNode>) {
  const { task, phases, pending, lead } = bar;
  const summary = reachedMilestones(task.milestones)
    .map(({ label, at }) => `${label} ${formatDate(at)}`)
    .join(", ");
  return (
    <div
      data-dimmed={dimmed || undefined}
      className={cn("relative h-full w-full", dimmed && "opacity-40")}
      title={`${workspace === null ? "" : `${workspace} `}#${task.id} ${task.title}\n${summary}`}
    >
      <Handle type="target" position={Position.Left} />
      {lead !== null && (
        // The wait before the bar: from the planned time, through the time it became ready.
        <div
          aria-hidden
          className="pointer-events-none absolute top-1/2"
          style={{ left: lead.x, width: -lead.x }}
        >
          <div className="absolute inset-x-0 top-0 border-t border-dashed border-muted-foreground/70" />
          {lead.planned && (
            <span className="absolute -left-1 -top-1 size-2 rotate-45 border border-muted-foreground bg-background" />
          )}
          {lead.readyX !== null && (
            <span className="absolute -top-1 h-2 border-l-2 border-emerald-500" style={{ left: lead.readyX - lead.x }} />
          )}
        </div>
      )}
      <div
        className={cn(
          "absolute inset-0 overflow-hidden rounded-md border bg-card shadow-sm",
          selected && "ring-2 ring-primary",
          pending && "border-dashed border-muted-foreground/60 shadow-none",
          isPaused(task) && "opacity-60",
        )}
      >
        <div className="absolute inset-x-0 bottom-0 h-1.5">
          {phases.map((phase) => (
            <span
              key={phase.x}
              className={cn("absolute inset-y-0", phaseStyles[phase.phase])}
              style={{ left: phase.x, width: phase.width }}
            />
          ))}
        </div>
      </div>
      {/* A bar narrower than its label lets the label run past its right end. */}
      <div
        className={cn(
          "relative flex items-center gap-1.5 whitespace-nowrap px-1.5 pt-1 text-xs text-card-foreground",
          isPaused(task) && "opacity-60",
        )}
        style={{ width: "max-content", minWidth: "100%" }}
      >
        {workspace !== null && <span className="rounded-sm bg-muted px-1 text-muted-foreground">{workspace}</span>}
        <span className="tabular-nums text-muted-foreground">#{task.id}</span>
        <span className="font-medium">{task.title}</span>
        <span className={cn("ml-auto shrink-0 rounded-full px-1.5 font-medium", statusStyle)}>{statusLabel}</span>
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
