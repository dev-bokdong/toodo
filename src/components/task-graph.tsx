import { useState } from "react";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import {
  Background,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { Button } from "@/components/ui/button";
import type { Task, TaskStatus, WorkspaceId } from "@/lib/bindings";
import { useAddDependency, useRemoveDependency } from "@/lib/queries";
import {
  canAddDependency,
  dependencyEdges,
  dependencyEditableStatuses,
  layoutTaskGraph,
  taskNodeSize,
} from "@/lib/task-graph";
import { cn } from "@/lib/utils";

type TaskNode = Node<{ task: Task; statusLabel: string; statusStyle: string }, "task">;

const nodeTypes = { task: TaskNodeView };

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
  const dependencies = dependencyEdges(tasks);
  // Status changes keep the layout; only added or removed tasks and edges move nodes.
  const shape = [tasks.map((task) => task.id), dependencies.map((edge) => edge.id)];
  const layout = useQuery({
    queryKey: ["task-graph-layout", workspaceId, shape],
    queryFn: () => layoutTaskGraph(tasks),
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });
  const changing = addDependency.isPending || removeDependency.isPending;
  const editable = (target: number) =>
    dependencyEditableStatuses.includes(tasks.find((task) => task.id === target)?.status ?? "done");

  const nodes: TaskNode[] = tasks.map((task) => ({
    id: String(task.id),
    type: "task",
    position: layout.data?.get(task.id) ?? { x: 0, y: 0 },
    data: { task, statusLabel: statusLabels[task.status], statusStyle: statusStyles[task.status] },
    deletable: false,
    ...taskNodeSize,
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
    <section aria-label="Dependency graph" className="flex flex-col gap-2">
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
      {(addDependency.error ?? removeDependency.error ?? layout.error) && (
        <p role="alert" className="text-xs text-destructive">
          {(addDependency.error ?? removeDependency.error ?? layout.error)?.message}
        </p>
      )}
      <div className="h-[28rem] rounded-lg border">
        {layout.data ? (
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            nodesDraggable={false}
            nodesConnectable={!changing}
            isValidConnection={(c) => canAddDependency(tasks, Number(c.source), Number(c.target))}
            onConnect={connect}
            onEdgesChange={changeEdges}
            onEdgesDelete={removeEdges}
            deleteKeyCode={["Backspace", "Delete"]}
            colorMode="system"
            fitView
            proOptions={{ hideAttribution: true }}
          >
            <Background />
            <Controls showInteractive={false} />
          </ReactFlow>
        ) : (
          <p className="p-3 text-sm text-muted-foreground">Laying out tasks...</p>
        )}
      </div>
    </section>
  );
}

function TaskNodeView({ data: { task, statusLabel, statusStyle } }: NodeProps<TaskNode>) {
  return (
    <div
      className={cn(
        "flex h-full w-full flex-col gap-1 rounded-lg border bg-card px-2.5 py-2 text-card-foreground shadow-sm",
        task.status === "cancelled" && "opacity-60",
      )}
    >
      <Handle type="target" position={Position.Left} />
      <div className="flex items-center gap-1.5 text-xs">
        <span className="tabular-nums text-muted-foreground">#{task.id}</span>
        <span className="rounded-md border px-1 font-medium">{task.taskType}</span>
        <span className={cn("ml-auto rounded-full px-1.5 font-medium", statusStyle)}>{statusLabel}</span>
      </div>
      <span className="line-clamp-2 text-sm font-medium leading-tight" title={task.title}>
        {task.title}
      </span>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
