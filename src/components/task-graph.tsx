import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { XIcon } from "lucide-react";
import { compactSelectClass } from "@/components/workspace-list";
import {
  BaseEdge,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  useStore,
  useViewport,
  type Connection,
  type Edge,
  type EdgeChange,
  type EdgeProps,
  type Node,
  type NodeChange,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { AgentBadge } from "@/components/agent-badge";
import { PrerequisiteConfirmation } from "@/components/prerequisite-confirmation";
import { TaskTypeBadge } from "@/components/task-type-badge";
import { Button } from "@/components/ui/button";
import type { AgentStatus, Task, TaskId, Timestamp } from "@/lib/bindings";
import { useAddDependency, useAgentStatuses, useRemoveDependency } from "@/lib/queries";
import {
  canAddDependency,
  dependencyEdges,
  dependencyEditableStatuses,
  isPaused,
  layoutTimeline,
  relatedTasks,
  pausesStartedTask,
  timelineAxis,
  timelineEdgePath,
  timelineScales,
  timelineYears,
  yearPeriod,
  type TimelineBar,
  type TimelineLayout,
  type TimelinePhase,
  type TimelineScale,
} from "@/lib/task-graph";
import { useThemePreference } from "@/lib/theme";
import { formatDuration } from "@/lib/review";
import { activeSeconds, formatDate, reachedMilestones } from "@/lib/timeline";
import { cn } from "@/lib/utils";

type BarNode = Node<
  {
    bar: TimelineBar;
    statusLabel: string;
    statusStyle: string;
    ready: boolean;
    workspace: string | null;
    dimmed: boolean;
    agentStatus: AgentStatus | null;
  },
  "bar"
>;

const nodeTypes = { bar: TimelineBarView };
const edgeTypes = { timeline: TimelineEdge };

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
/** Width of the floating task details sheet, and its distance from the window edges. */
const sheetWidth = 480;
const sheetMargin = 16;
/** Width of the hover card of a bar. */
const tooltipWidth = 288;
/** Keys that move the selection between timeline rows, and where each one goes. */
const rowKeys: Record<string, "previous" | "next" | "first" | "last"> = {
  ArrowUp: "previous",
  ArrowDown: "next",
  Home: "first",
  End: "last",
};

const scaleLabels: Record<TimelineScale, string> = { hour: "Hour", day: "Day", week: "Week", month: "Month" };

const phaseStyles: Record<TimelinePhase, string> = {
  in_progress: "bg-blue-500",
  in_review: "bg-violet-500",
  paused: "bg-neutral-400",
};

const phaseLabels: Record<TimelinePhase, string> = {
  in_progress: "In progress",
  in_review: "In review",
  paused: "Paused",
};

/** The bar of a finished task is tinted green; its phase strip still shows how the work went. */
const doneBarStyle = "border-emerald-500 bg-emerald-100 dark:bg-emerald-950";

/** A task not started yet whose prerequisites are all done is drawn solid; one still blocked, dashed. */
const readyBarStyle = "border-amber-500 shadow-none";

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
  ready,
  scale,
  onScaleChange,
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
  /** Whether a planned task can be started now; its bar is drawn solid instead of dashed. */
  ready: (task: Task) => boolean;
  scale: TimelineScale;
  onScaleChange: (scale: TimelineScale) => void;
  /** Details of the task selected on the timeline, with its actions. */
  renderDetail: (task: Task) => ReactNode;
}) {
  const addDependency = useAddDependency();
  const removeDependency = useRemoveDependency();
  const agentStatuses = useAgentStatuses();
  const [selectedEdgeIds, setSelectedEdgeIds] = useState<ReadonlySet<string>>(new Set());
  const [selectedTaskId, setSelectedTaskId] = useState<TaskId | null>(null);
  // A dependency drawn onto a started task that it would pause, waiting for confirmation.
  const [confirmingEdge, setConfirmingEdge] = useState<{ id: TaskId; dependsOn: TaskId } | null>(null);
  const [year, setYear] = useState<number | null>(null);
  // The bar under the pointer, with its place on screen for the hover card.
  const [hovered, setHovered] = useState<{ id: TaskId; rect: DOMRect } | null>(null);
  const colorMode = useThemePreference();
  const container = useRef<HTMLDivElement>(null);
  const flow = useRef<ReactFlowInstance<BarNode> | null>(null);
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
  // The panel fills the space the window leaves it, so its height is measured, not derived from the rows.
  const [panelHeight, setPanelHeight] = useState(0);
  useEffect(() => {
    const element = container.current;
    if (element === null) {
      return;
    }
    const observer = new ResizeObserver(() => setPanelHeight(element.clientHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
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
      ready: ready(bar.task),
      workspace: workspaceLabel(bar.task),
      dimmed: dimmed.has(bar.task.id) || !inChain(bar.task.id),
      agentStatus: agentStatusOf(bar.task),
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
    type: "timeline",
    selected: selectedEdgeIds.has(edge.id),
    deletable: editable(edge.target),
    markerEnd: { type: MarkerType.ArrowClosed },
    style: inChain(edge.source) && inChain(edge.target) ? undefined : { opacity: 0.2 },
  }));
  function agentStatusOf(task: Task): AgentStatus | null {
    if (task.status !== "in_progress" && task.status !== "in_review") {
      return null;
    }
    return agentStatuses.data?.find((entry) => entry.taskId === task.id)?.status ?? null;
  }

  const selected = dependencies.filter((edge) => selectedEdgeIds.has(edge.id) && editable(edge.target));
  const hoveredBar = layout.bars.find((bar) => bar.task.id === hovered?.id);

  function showLatest(instance: ReactFlowInstance<BarNode>) {
    flow.current = instance;
    const visible = container.current?.clientWidth ?? 0;
    void instance.setViewport({ x: Math.min(16, visible - layout.width - 16), y: 0, zoom: 1 });
  }

  function connect({ source, target }: Connection) {
    const edge = { id: Number(target), dependsOn: Number(source) };
    const task = allTasks.find((other) => other.id === edge.id);
    const prerequisite = allTasks.find((other) => other.id === edge.dependsOn);
    if (task !== undefined && pausesStartedTask(task, prerequisite)) {
      setConfirmingEdge(edge);
    } else {
      addDependency.mutate(edge);
    }
  }

  function removeEdges(removed: { source: string; target: string }[]) {
    for (const edge of removed) {
      removeDependency.mutate({ id: Number(edge.target), dependsOn: Number(edge.source) });
    }
    setSelectedEdgeIds(new Set());
  }

  /**
   * Pans the timeline so the bar of a task is on screen: clear of the floating details sheet
   * when `behindSheet`, and of the axis header and the panel's bottom edge.
   */
  function reveal(id: TaskId, behindSheet: boolean) {
    const instance = flow.current;
    const box = container.current?.getBoundingClientRect();
    const bar = layout.bars.find((candidate) => candidate.task.id === id);
    if (instance === null || box === undefined || bar === undefined) {
      return;
    }
    const sheetLeft = window.innerWidth - sheetMargin - Math.min(sheetWidth, window.innerWidth - 2 * sheetMargin);
    const visibleRight = (behindSheet ? Math.min(box.right, sheetLeft) : box.right) - 16;
    const { x, y, zoom } = instance.getViewport();
    const left = box.left + x + bar.x;
    // Shifts that put the bar's left end at the panel's left edge, or its right end at the visible right.
    const toLeftEdge = left - box.left - 16;
    const toRightEnd = left + bar.width - visibleRight;
    let shift = 0;
    if (toRightEnd > 0) {
      // Move the bar left until its right end is clear, but never past the panel's left edge.
      shift = Math.max(Math.min(toRightEnd, toLeftEdge), 0);
    } else if (left + bar.width < box.left + 16) {
      // A bar entirely off the left edge comes back with its latest part showing.
      shift = Math.max(toLeftEdge, toRightEnd);
    }
    const top = rowsTop + bar.row * rowHeight;
    let nextY = y;
    if (top + y < rowsTop) {
      nextY = rowsTop - top;
    } else if (top + rowHeight + y > box.height) {
      nextY = box.height - top - rowHeight;
    }
    if (shift !== 0 || nextY !== y) {
      void instance.setViewport({ x: x - shift, y: nextY, zoom }, { duration: 200 });
    }
  }

  function select(id: TaskId) {
    setSelectedTaskId(id);
    setSelectedEdgeIds(new Set());
    setHovered(null);
    reveal(id, true);
  }

  /** Moves the selection, and the keyboard focus, to the row above or below, or to the first or last row. */
  function moveSelection(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    const move = rowKeys[event.key];
    // Keys typed into the details sheet, or anywhere outside the bars, keep their usual meaning.
    const node = target.closest<HTMLElement>(".react-flow__node");
    if (move === undefined || !container.current?.contains(target) || (node === null && selectedTask === undefined)) {
      return;
    }
    const rows = [...layout.bars].sort((a, b) => a.row - b.row);
    const current = rows.findIndex((bar) => String(bar.task.id) === (node?.dataset.id ?? String(selectedTask?.id)));
    const index = {
      previous: current <= 0 ? 0 : current - 1,
      next: current === -1 ? 0 : Math.min(current + 1, rows.length - 1),
      first: 0,
      last: rows.length - 1,
    }[move];
    const id = rows[index]?.task.id;
    if (id === undefined) {
      return;
    }
    event.preventDefault();
    container.current
      .querySelector<HTMLElement>(`.react-flow__node[data-id="${id}"]`)
      ?.focus({ preventScroll: true });
    select(id);
  }

  function changeNodes(changes: NodeChange<BarNode>[]) {
    const picked = changes.filter((change) => change.type === "select" && change.selected).pop();
    if (picked !== undefined && picked.type === "select") {
      select(Number(picked.id));
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
    <section aria-label="Task timeline" className="flex flex-1 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <div role="group" aria-label="Timeline scale" className="flex gap-1">
          {timelineScales.map((option) => (
            <Button
              key={option}
              size="xs"
              variant={scale === option ? "secondary" : "ghost"}
              aria-pressed={scale === option}
              onClick={() => onScaleChange(option)}
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
            Blocked
          </li>
          <li className="flex items-center gap-1">
            <span className={cn("h-2 w-3 rounded-sm border", readyBarStyle)} />
            Ready to start
          </li>
          {(Object.keys(phaseLabels) as TimelinePhase[]).map((phase) => (
            <li key={phase} className="flex items-center gap-1">
              <span className={cn("h-1.5 w-3 rounded-full", phaseStyles[phase])} />
              {phaseLabels[phase]}
            </li>
          ))}
          <li className="flex items-center gap-1">
            <span className={cn("h-2 w-3 rounded-sm border", doneBarStyle)} />
            Done
          </li>
        </ul>
      </div>
      <div className="flex min-h-7 flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>
          Tasks not started yet wait at the current time, after their prerequisites. Drag from a task's right handle
          to another task to make it a prerequisite. With a task selected, use the arrow keys to move between rows.
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
      {confirmingEdge !== null && (
        <PrerequisiteConfirmation
          taskId={confirmingEdge.id}
          prerequisite={`#${confirmingEdge.dependsOn}`}
          pending={addDependency.isPending}
          onConfirm={() => addDependency.mutate(confirmingEdge, { onSettled: () => setConfirmingEdge(null) })}
          onCancel={() => setConfirmingEdge(null)}
        />
      )}
      {(addDependency.error ?? removeDependency.error) && (
        <p role="alert" className="text-xs text-destructive">
          {(addDependency.error ?? removeDependency.error)?.message}
        </p>
      )}
      <div
        className="flex min-h-40 flex-1 gap-2"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            setSelectedTaskId(null);
          } else {
            moveSelection(event);
          }
        }}
        onFocus={(event) => {
          // A bar reached with Tab comes into view; a selected one also clears the details sheet.
          const id = (event.target as HTMLElement).closest<HTMLElement>(".react-flow__node")?.dataset.id;
          if (id !== undefined) {
            reveal(Number(id), Number(id) === selectedTask?.id);
          }
        }}
      >
        <div
          ref={container}
          className="min-w-0 flex-1 rounded-lg border"
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
              edgeTypes={edgeTypes}
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
              onNodeMouseEnter={(event, node) =>
                setHovered({ id: Number(node.id), rect: event.currentTarget.getBoundingClientRect() })
              }
              onNodeMouseLeave={() => setHovered(null)}
              onMoveStart={() => setHovered(null)}
              autoPanOnNodeFocus={false}
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
        {hovered !== null && hoveredBar !== undefined && (
          <TimelineTooltip
            task={hoveredBar.task}
            anchor={hovered.rect}
            bounds={container.current?.getBoundingClientRect() ?? hovered.rect}
            workspace={workspaceLabel(hoveredBar.task)}
            statusLabel={statusLabel(hoveredBar.task)}
            statusStyle={statusStyle(hoveredBar.task)}
            agentStatus={agentStatusOf(hoveredBar.task)}
            now={now}
          />
        )}
        {selectedTask !== undefined && (
          // A floating sheet over the window's right side: the timeline keeps its full width and
          // the details get room to wrap instead of being cut off.
          <aside
            aria-label="Task details"
            className="fixed inset-y-4 right-4 z-40 flex flex-col overflow-hidden rounded-xl border bg-card shadow-xl"
            style={{ width: `min(${sheetWidth}px, calc(100vw - ${2 * sheetMargin}px))` }}
          >
            <div className="flex shrink-0 items-center justify-between border-b px-4 py-2">
              <h3 className="text-xs font-medium text-muted-foreground">Task details</h3>
              <Button size="icon-xs" variant="ghost" aria-label="Close details" onClick={() => setSelectedTaskId(null)}>
                <XIcon />
              </Button>
            </div>
            <ul className="min-h-0 flex-1 overflow-y-auto">{renderDetail(selectedTask)}</ul>
          </aside>
        )}
        {layout.unplaced.length > 0 && (
              <aside aria-label="Tasks without a planned date" className="flex w-56 shrink-0 flex-col gap-1 text-xs">
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

/**
 * The hover card of a bar: what the bar's label may cut off, and when the task reached each
 * milestone. It sits below the bar, or above it near the bottom of the window.
 */
function TimelineTooltip({
  task,
  anchor,
  bounds,
  workspace,
  statusLabel,
  statusStyle,
  agentStatus,
  now,
}: {
  task: Task;
  anchor: DOMRect;
  bounds: DOMRect;
  workspace: string | null;
  statusLabel: string;
  statusStyle: string;
  agentStatus: AgentStatus | null;
  now: Timestamp;
}) {
  const milestones = reachedMilestones(task.milestones);
  const active = activeSeconds(task.timeline, now);
  // A bar may start far off screen; the card starts where the visible part of the bar does.
  const left = Math.max(
    Math.min(Math.max(anchor.left, bounds.left), window.innerWidth - tooltipWidth - 8),
    8,
  );
  const below = anchor.bottom < window.innerHeight * 0.6;
  return (
    <div
      role="tooltip"
      aria-label={`Summary of #${task.id}`}
      className="pointer-events-none fixed z-50 flex flex-col gap-1.5 rounded-lg border bg-popover p-2.5 text-xs text-popover-foreground shadow-lg"
      style={{
        left,
        width: tooltipWidth,
        ...(below ? { top: anchor.bottom + 6 } : { bottom: window.innerHeight - anchor.top + 6 }),
      }}
    >
      <div className="flex items-center gap-1.5">
        {workspace !== null && <span className="rounded-sm bg-muted px-1 text-muted-foreground">{workspace}</span>}
        <TaskTypeBadge type={task.taskType} className="px-1 leading-4" />
        <span className="tabular-nums text-muted-foreground">#{task.id}</span>
        <span className={cn("ml-auto shrink-0 rounded-full px-1.5 font-medium", statusStyle)}>{statusLabel}</span>
        {agentStatus !== null && <AgentBadge taskId={task.id} status={agentStatus} className="px-1.5 py-0" />}
      </div>
      <p className="font-medium break-words">{task.title}</p>
      {milestones.length > 0 && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
          {milestones.map(({ label, at }) => (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="tabular-nums">{formatDate(at)}</dd>
            </div>
          ))}
          {active !== null && (
            <>
              <dt className="text-muted-foreground">Active</dt>
              <dd className="tabular-nums">{formatDuration(active)}</dd>
            </>
          )}
        </dl>
      )}
      <p className="text-muted-foreground">Click for details</p>
    </div>
  );
}

/** A dependency line routed between the rows, so it shows even when the bars meet in time. */
function TimelineEdge({ sourceX, sourceY, targetX, targetY, markerEnd, style, interactionWidth }: EdgeProps) {
  const path = timelineEdgePath({ x: sourceX, y: sourceY }, { x: targetX, y: targetY }, rowHeight);
  return (
    <>
      {/* A halo in the background color keeps the line apart from bar borders and crossing lines. */}
      <path d={path} className="fill-none stroke-background" strokeWidth={5} style={{ opacity: style?.opacity }} />
      <BaseEdge path={path} markerEnd={markerEnd} style={style} interactionWidth={interactionWidth} />
    </>
  );
}

function TimelineBarView({
  data: { bar, statusLabel, statusStyle, ready, workspace, dimmed, agentStatus },
  selected,
}: NodeProps<BarNode>) {
  const { task, phases, pending, lead } = bar;
  // A paused task whose new prerequisite is done is ready again and drawn like a ready one.
  const faded = isPaused(task) && !ready;
  return (
    <div data-dimmed={dimmed || undefined} className={cn("relative h-full w-full", dimmed && "opacity-40")}>
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
          ready ? readyBarStyle : pending && "border-dashed border-muted-foreground/60 shadow-none",
          task.status === "done" && doneBarStyle,
          faded && "opacity-60",
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
          faded && "opacity-60",
        )}
        style={{ width: "max-content", minWidth: "100%" }}
      >
        {workspace !== null && <span className="rounded-sm bg-muted px-1 text-muted-foreground">{workspace}</span>}
        <TaskTypeBadge type={task.taskType} className="bg-card px-1 leading-4" />
        <span className="tabular-nums text-muted-foreground">#{task.id}</span>
        <span className="font-medium">{task.title}</span>
        <span className={cn("ml-auto shrink-0 rounded-full px-1.5 font-medium", statusStyle)}>{statusLabel}</span>
        {agentStatus !== null && <AgentBadge taskId={task.id} status={agentStatus} className="px-1.5 py-0" />}
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
