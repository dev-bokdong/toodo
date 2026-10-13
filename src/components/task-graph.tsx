import { useRef, useState } from "react";
import {
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  ViewportPortal,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeProps,
  type ReactFlowInstance,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { Button } from "@/components/ui/button";
import type { Task, TaskStatus, WorkspaceId } from "@/lib/bindings";
import { useAddDependency, useRemoveDependency } from "@/lib/queries";
import {
  canAddDependency,
  dependencyEdges,
  dependencyEditableStatuses,
  layoutTimeline,
  timelineScales,
  timelineYears,
  yearPeriod,
  type TimelineBar,
  type TimelinePhase,
  type TimelineScale,
} from "@/lib/task-graph";
import { formatDate, reachedMilestones } from "@/lib/timeline";
import { cn } from "@/lib/utils";

type BarNode = Node<{ bar: TimelineBar; statusLabel: string; statusStyle: string }, "bar">;

const nodeTypes = { bar: TimelineBarView };

const axisHeight = 28;
const laneHeight = 52;
const barHeight = 40;

const scaleLabels: Record<TimelineScale, string> = { day: "Day", week: "Week", month: "Month" };

const phaseStyles: Record<TimelinePhase, string> = {
  waiting: "bg-muted-foreground/30",
  in_progress: "bg-blue-500",
  in_review: "bg-violet-500",
};

const phaseLabels: Record<TimelinePhase, string> = {
  waiting: "Waiting",
  in_progress: "In progress",
  in_review: "In review",
};

export function TaskGraph({
  workspaceId,
  tasks,
  statusLabels,
  statusStyles,
}: {
  workspaceId: WorkspaceId;
  tasks: Task[];
  statusLabels: Record<TaskStatus, string>;
  statusStyles: Record<TaskStatus, string>;
}) {
  const addDependency = useAddDependency();
  const removeDependency = useRemoveDependency();
  const [selectedEdgeIds, setSelectedEdgeIds] = useState<ReadonlySet<string>>(new Set());
  const [scale, setScale] = useState<TimelineScale>("week");
  const [year, setYear] = useState<number | null>(null);
  const container = useRef<HTMLDivElement>(null);
  const now = Math.floor(Date.now() / 1000);
  const layout = layoutTimeline(tasks, { now, scale, period: year === null ? null : yearPeriod(year) });
  const placed = new Set(layout.bars.map((bar) => bar.task.id));
  const dependencies = dependencyEdges(tasks).filter((edge) => placed.has(edge.source) && placed.has(edge.target));
  const changing = addDependency.isPending || removeDependency.isPending;
  const editable = (target: number) =>
    dependencyEditableStatuses.includes(tasks.find((task) => task.id === target)?.status ?? "done");
  const height = axisHeight + layout.lanes * laneHeight;

  const nodes: BarNode[] = layout.bars.map((bar) => ({
    id: String(bar.task.id),
    type: "bar",
    position: { x: bar.x, y: axisHeight + bar.lane * laneHeight },
    data: { bar, statusLabel: statusLabels[bar.task.status], statusStyle: statusStyles[bar.task.status] },
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
  }));
  const selected = dependencies.filter((edge) => selectedEdgeIds.has(edge.id) && editable(edge.target));

  function showLatest(instance: ReactFlowInstance<BarNode>) {
    const visible = container.current?.clientWidth ?? 0;
    void instance.setViewport({ x: Math.min(16, visible - layout.width - 16), y: 8, zoom: 1 });
  }

  function connect({ source, target }: Connection) {
    addDependency.mutate({ workspaceId, id: Number(target), dependsOn: Number(source) });
  }

  function removeEdges(removed: { source: string; target: string }[]) {
    for (const edge of removed) {
      removeDependency.mutate({ workspaceId, id: Number(edge.target), dependsOn: Number(edge.source) });
    }
    setSelectedEdgeIds(new Set());
  }

  function changeEdges(changes: EdgeChange[]) {
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
          className="h-6 rounded-md border bg-background px-1.5"
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
          {(Object.keys(phaseLabels) as TimelinePhase[]).map((phase) => (
            <li key={phase} className="flex items-center gap-1">
              <span className={cn("h-1.5 w-3 rounded-full", phaseStyles[phase])} />
              {phaseLabels[phase]}
            </li>
          ))}
        </ul>
      </div>
      <div className="flex min-h-7 flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>Drag from a task's right handle to another task to make it a prerequisite.</span>
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
      <div className="flex gap-2">
        <div
          ref={container}
          className="min-w-0 flex-1 rounded-lg border"
          style={{ height: Math.min(Math.max(height + 32, 160), 448) }}
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
              nodesDraggable={false}
              nodesConnectable={!changing}
              isValidConnection={(c) => canAddDependency(tasks, Number(c.source), Number(c.target))}
              onConnect={connect}
              onEdgesChange={changeEdges}
              onEdgesDelete={removeEdges}
              deleteKeyCode={["Backspace", "Delete"]}
              colorMode="system"
              proOptions={{ hideAttribution: true }}
            >
              <ViewportPortal>
                <div aria-hidden className="pointer-events-none absolute left-0 top-0 -z-10">
                  {layout.ticks.map((tick) => (
                    <div
                      key={tick.at}
                      className="absolute top-0 whitespace-nowrap border-l border-border text-[10px] text-muted-foreground"
                      style={{ left: tick.x, height }}
                    >
                      <span className="pl-1">{tick.label}</span>
                    </div>
                  ))}
                  {layout.nowX !== null && (
                    <div
                      className="absolute top-0 whitespace-nowrap border-l-2 border-dashed border-red-500 text-[10px] text-red-500"
                      style={{ left: layout.nowX, height }}
                    >
                      <span className="absolute bottom-0 pl-1">Today</span>
                    </div>
                  )}
                </div>
              </ViewportPortal>
            </ReactFlow>
          )}
        </div>
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
                  <span className={cn("ml-auto shrink-0 rounded-full px-1.5", statusStyles[task.status])}>
                    {statusLabels[task.status]}
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

function TimelineBarView({ data: { bar, statusLabel, statusStyle } }: NodeProps<BarNode>) {
  const { task, phases } = bar;
  const summary = reachedMilestones(task.milestones)
    .map(({ label, at }) => `${label} ${formatDate(at)}`)
    .join(", ");
  return (
    <div
      className={cn(
        "relative flex h-full w-full items-start overflow-hidden rounded-md border bg-card px-1.5 pt-1 text-card-foreground shadow-sm",
        task.status === "cancelled" && "opacity-60",
      )}
      title={`#${task.id} ${task.title}\n${summary}`}
    >
      <Handle type="target" position={Position.Left} />
      <div className="flex min-w-0 flex-1 items-center gap-1.5 text-xs">
        <span className="tabular-nums text-muted-foreground">#{task.id}</span>
        <span className="truncate font-medium">{task.title}</span>
        <span className={cn("ml-auto shrink-0 rounded-full px-1.5 font-medium", statusStyle)}>{statusLabel}</span>
      </div>
      <div className="absolute inset-x-0 bottom-0 h-1.5">
        {phases.map((phase) => (
          <span
            key={phase.phase}
            className={cn("absolute inset-y-0", phaseStyles[phase.phase])}
            style={{ left: phase.x, width: phase.width }}
          />
        ))}
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
