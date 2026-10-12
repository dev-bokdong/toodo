import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import type { Workspace, WorkspaceId, WorkspaceLocation } from "@/lib/bindings";
import { useCreateWorkspace, useWorkspaces } from "@/lib/queries";
import { cn } from "@/lib/utils";

export const inputClass =
  "h-8 w-full rounded-lg border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

export function describeLocation(location: WorkspaceLocation): string {
  return location.kind === "local" ? location.path : `${location.machine}:${location.path}`;
}

type WorkspaceListProps = {
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
};

export function WorkspaceList({ selectedId, onSelect }: WorkspaceListProps) {
  const workspaces = useWorkspaces();
  const [creating, setCreating] = useState(false);

  return (
    <section aria-labelledby="workspaces-heading" className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 id="workspaces-heading" className="text-xs font-medium uppercase text-muted-foreground">
          Workspaces
        </h2>
        <Button variant="ghost" size="xs" onClick={() => setCreating((open) => !open)}>
          {creating ? "Cancel" : "New"}
        </Button>
      </div>

      {creating && (
        <NewWorkspaceForm
          onCreated={(workspace) => {
            setCreating(false);
            onSelect(workspace.id);
          }}
        />
      )}

      {workspaces.isPending && <p className="text-sm text-muted-foreground">Loading workspaces...</p>}
      {workspaces.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load workspaces: {workspaces.error.message}
        </p>
      )}
      {workspaces.data?.length === 0 && !creating && (
        <p className="text-sm text-muted-foreground">No workspaces yet.</p>
      )}
      {workspaces.data && workspaces.data.length > 0 && (
        <ul className="flex flex-col gap-1 overflow-y-auto">
          {workspaces.data.map((workspace) => (
            <li key={workspace.id}>
              <button
                type="button"
                aria-current={workspace.id === selectedId ? "true" : undefined}
                onClick={() => onSelect(workspace.id)}
                className={cn(
                  "flex w-full flex-col rounded-lg px-2.5 py-1.5 text-left hover:bg-sidebar-accent",
                  workspace.id === selectedId && "bg-sidebar-accent",
                )}
              >
                <span className="truncate text-sm font-medium">{workspace.name}</span>
                <span className="truncate text-xs text-muted-foreground">
                  {describeLocation(workspace.location)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function NewWorkspaceForm({ onCreated }: { onCreated: (workspace: Workspace) => void }) {
  const createWorkspace = useCreateWorkspace();
  const [name, setName] = useState("");
  const [machine, setMachine] = useState("");
  const [path, setPath] = useState("");
  const [baseBranch, setBaseBranch] = useState("main");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const location: WorkspaceLocation = machine.trim()
      ? { kind: "ssh", machine: machine.trim(), path }
      : { kind: "local", path };
    createWorkspace.mutate(
      { name, location, defaultBaseBranch: baseBranch },
      { onSuccess: onCreated },
    );
  }

  return (
    <form onSubmit={submit} aria-label="New workspace" className="flex flex-col gap-2 rounded-lg border p-2">
      <input
        className={inputClass}
        placeholder="Name"
        aria-label="Name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        required
      />
      <input
        className={inputClass}
        placeholder="Path"
        aria-label="Path"
        value={path}
        onChange={(e) => setPath(e.target.value)}
        required
      />
      <input
        className={inputClass}
        placeholder="SSH machine (empty for local)"
        aria-label="SSH machine"
        value={machine}
        onChange={(e) => setMachine(e.target.value)}
      />
      <input
        className={inputClass}
        placeholder="Base branch"
        aria-label="Base branch"
        value={baseBranch}
        onChange={(e) => setBaseBranch(e.target.value)}
        required
      />
      {createWorkspace.isError && (
        <p role="alert" className="text-xs text-destructive">
          {createWorkspace.error.message}
        </p>
      )}
      <Button type="submit" size="sm" disabled={createWorkspace.isPending}>
        Create workspace
      </Button>
    </form>
  );
}
