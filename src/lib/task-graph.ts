import ELK from "elkjs/lib/elk.bundled.js";
import type { Task, TaskId, TaskStatus } from "@/lib/bindings";

export const taskNodeSize = { width: 224, height: 76 };

/** A dependency drawn from the prerequisite (`source`) to the task that waits on it (`target`). */
export type DependencyEdge = { id: string; source: TaskId; target: TaskId };

export type Position = { x: number; y: number };

/** Statuses whose dependencies can still change; the backend rejects edits for the others. */
export const dependencyEditableStatuses: TaskStatus[] = ["planned", "ready"];

const elk = new ELK();

export function edgeId(source: TaskId, target: TaskId): string {
  return `${source}->${target}`;
}

/** Dependency edges among `tasks`; dependencies on tasks outside the list are skipped. */
export function dependencyEdges(tasks: Task[]): DependencyEdge[] {
  const ids = new Set(tasks.map((task) => task.id));
  return tasks.flatMap((task) =>
    task.dependsOn
      .filter((dependency) => ids.has(dependency))
      .map((dependency) => ({ id: edgeId(dependency, task.id), source: dependency, target: task.id })),
  );
}

/**
 * Whether `target` may gain a dependency on `source`. Cycles are left to the backend,
 * which owns the DAG rules and reports them as a command error.
 */
export function canAddDependency(tasks: Task[], source: TaskId, target: TaskId): boolean {
  if (source === target) {
    return false;
  }
  const from = tasks.find((task) => task.id === source);
  const to = tasks.find((task) => task.id === target);
  return (
    from !== undefined &&
    to !== undefined &&
    from.status !== "cancelled" &&
    dependencyEditableStatuses.includes(to.status) &&
    !to.dependsOn.includes(source)
  );
}

/** Lays the graph out left to right, prerequisites first; returns the top-left corner of each node. */
export async function layoutTaskGraph(tasks: Task[]): Promise<Map<TaskId, Position>> {
  const graph = await elk.layout({
    id: "root",
    layoutOptions: {
      "elk.algorithm": "layered",
      "elk.direction": "RIGHT",
      "elk.spacing.nodeNode": "24",
      "elk.layered.spacing.nodeNodeBetweenLayers": "56",
    },
    children: tasks.map((task) => ({ id: String(task.id), ...taskNodeSize })),
    edges: dependencyEdges(tasks).map((edge) => ({
      id: edge.id,
      sources: [String(edge.source)],
      targets: [String(edge.target)],
    })),
  });
  return new Map(
    (graph.children ?? []).map((node) => [Number(node.id), { x: node.x ?? 0, y: node.y ?? 0 }]),
  );
}
