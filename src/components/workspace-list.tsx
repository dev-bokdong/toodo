import { useState, type FormEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { RemoteFolderBrowser, joinPosix } from "@/components/remote-folder-browser";
import { Button } from "@/components/ui/button";
import type { Workspace, WorkspaceId, WorkspaceLocation, WorkspaceSource } from "@/lib/bindings";
import { useAddSshHost, useCreateWorkspace, useSshHosts, useWorkspaces } from "@/lib/queries";
import { cn } from "@/lib/utils";

export const inputClass =
  "h-8 w-full rounded-lg border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

export function describeLocation(location: WorkspaceLocation): string {
  return location.kind === "local" ? location.path : `${location.machine}:${location.path}`;
}

/** SSH host alias, or `null` for this computer. */
type MachineKey = string | null;

type Mode = WorkspaceSource["kind"];

function machineKeyOf(location: WorkspaceLocation): MachineKey {
  return location.kind === "local" ? null : location.machine;
}

function joinLocal(base: string, name: string): string {
  return /[\\/]$/.test(base) ? `${base}${name}` : `${base}\\${name}`;
}

function baseName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
}

function repositoryName(url: string): string {
  return baseName(url.trim().replace(/\.git$/, "")).split(":").pop() ?? "";
}

type WorkspaceListProps = {
  selectedId: WorkspaceId | null;
  onSelect: (id: WorkspaceId) => void;
};

export function WorkspaceList({ selectedId, onSelect }: WorkspaceListProps) {
  const workspaces = useWorkspaces();
  const sshHosts = useSshHosts();
  const [adding, setAdding] = useState<{ machine: MachineKey; mode: Mode } | null>(null);
  const [addingHost, setAddingHost] = useState(false);

  const remotes = [
    ...new Set([
      ...(sshHosts.data ?? []),
      ...(workspaces.data ?? []).flatMap((w) => (w.location.kind === "ssh" ? [w.location.machine] : [])),
    ]),
  ];
  const machines: MachineKey[] = [null, ...remotes];

  return (
    <section aria-labelledby="workspaces-heading" className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 id="workspaces-heading" className="text-xs font-medium uppercase text-muted-foreground">
          Workspaces
        </h2>
        <Button variant="ghost" size="xs" onClick={() => setAddingHost((open) => !open)}>
          {addingHost ? "Cancel" : "Add SSH host"}
        </Button>
      </div>

      {addingHost && <NewSshHostForm onAdded={() => setAddingHost(false)} />}
      {sshHosts.isError && (
        <p role="alert" className="text-xs text-destructive">
          Failed to read the SSH config: {sshHosts.error.message}
        </p>
      )}
      {workspaces.isPending && <p className="text-sm text-muted-foreground">Loading workspaces...</p>}
      {workspaces.isError && (
        <p role="alert" className="text-sm text-destructive">
          Failed to load workspaces: {workspaces.error.message}
        </p>
      )}
      {workspaces.data?.length === 0 && adding === null && (
        <p className="text-sm text-muted-foreground">No workspaces yet.</p>
      )}

      <div className="flex min-h-0 flex-col gap-3 overflow-y-auto">
        {workspaces.data &&
          machines.map((machine) => {
            const label = machine ?? "Local";
            const items = workspaces.data.filter((w) => machineKeyOf(w.location) === machine);
            const active = adding?.machine === machine ? adding.mode : null;
            return (
              <section key={label} aria-label={`Machine ${label}`} className="flex flex-col gap-1">
                <div className="flex items-center gap-1">
                  <h3 className="min-w-0 flex-1 truncate px-1 text-xs font-semibold">{label}</h3>
                  {(["clone", "open"] as const).map((mode) => (
                    <Button
                      key={mode}
                      size="xs"
                      variant={active === mode ? "secondary" : "ghost"}
                      aria-label={`${mode === "clone" ? "Clone" : "Open"} on ${label}`}
                      aria-pressed={active === mode}
                      onClick={() => setAdding(active === mode ? null : { machine, mode })}
                    >
                      {mode === "clone" ? "Clone" : "Open"}
                    </Button>
                  ))}
                </div>
                {active && (
                  <NewWorkspaceForm
                    key={active}
                    machine={machine}
                    mode={active}
                    onCreated={(workspace) => {
                      setAdding(null);
                      onSelect(workspace.id);
                    }}
                  />
                )}
                {items.length > 0 && (
                  <ul className="flex flex-col gap-1">
                    {items.map((workspace) => (
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
                          <span className="truncate text-xs text-muted-foreground">{workspace.location.path}</span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            );
          })}
      </div>
    </section>
  );
}

function NewSshHostForm({ onAdded }: { onAdded: () => void }) {
  const addHost = useAddSshHost();
  const [alias, setAlias] = useState("");
  const [hostName, setHostName] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    addHost.mutate(
      { alias, hostName, user: user.trim() || null, port: port.trim() ? Number(port) : null },
      { onSuccess: onAdded },
    );
  }

  return (
    <form onSubmit={submit} aria-label="New SSH host" className="flex flex-col gap-2 rounded-lg border p-2">
      <p className="text-xs text-muted-foreground">Saved as a Host entry in ~/.ssh/config.</p>
      <input className={inputClass} placeholder="Alias, e.g. devbox" aria-label="Host alias" value={alias} onChange={(e) => setAlias(e.target.value)} required />
      <input className={inputClass} placeholder="Host name or IP" aria-label="Host name" value={hostName} onChange={(e) => setHostName(e.target.value)} required />
      <div className="grid grid-cols-[1fr_5rem] gap-1">
        <input className={inputClass} placeholder="User (optional)" aria-label="User" value={user} onChange={(e) => setUser(e.target.value)} />
        <input className={inputClass} placeholder="Port" aria-label="Port" inputMode="numeric" pattern="[0-9]*" value={port} onChange={(e) => setPort(e.target.value)} />
      </div>
      {addHost.isError && (
        <p role="alert" className="text-xs text-destructive">
          {addHost.error.message}
        </p>
      )}
      <Button type="submit" size="sm" disabled={addHost.isPending}>
        {addHost.isPending ? "Adding..." : "Add host"}
      </Button>
    </form>
  );
}

type NewWorkspaceFormProps = {
  machine: MachineKey;
  mode: Mode;
  onCreated: (workspace: Workspace) => void;
};

function NewWorkspaceForm({ machine, mode, onCreated }: NewWorkspaceFormProps) {
  const createWorkspace = useCreateWorkspace();
  const [url, setUrl] = useState("");
  const [folder, setFolder] = useState("");
  const [name, setName] = useState<string | null>(null);
  const [baseBranch, setBaseBranch] = useState("");
  const [browsing, setBrowsing] = useState(false);
  const [pickError, setPickError] = useState<string | null>(null);

  const cloning = mode === "clone";
  const effectiveName = name ?? (cloning ? repositoryName(url) : baseName(folder));
  const folderLabel = cloning ? "Parent folder" : "Folder";
  const path =
    cloning && folder.trim() && effectiveName.trim()
      ? machine === null
        ? joinLocal(folder.trim(), effectiveName.trim())
        : joinPosix(folder.trim(), effectiveName.trim())
      : folder.trim();

  async function browse() {
    if (machine !== null) {
      setBrowsing((open) => !open);
      return;
    }
    setPickError(null);
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: folderLabel });
      if (typeof picked === "string") setFolder(picked);
    } catch (error) {
      setPickError(`Could not open the folder dialog: ${String(error)}`);
    }
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const location: WorkspaceLocation =
      machine === null ? { kind: "local", path } : { kind: "ssh", machine, path };
    const source: WorkspaceSource = cloning ? { kind: "clone", url } : { kind: "open" };
    createWorkspace.mutate(
      { name: effectiveName, location, source, defaultBaseBranch: baseBranch.trim() || null },
      { onSuccess: onCreated },
    );
  }

  return (
    <form
      onSubmit={submit}
      aria-label={`${cloning ? "Clone" : "Open"} workspace on ${machine ?? "Local"}`}
      className="flex flex-col gap-2 rounded-lg border p-2"
    >
      {cloning && (
        <input
          className={inputClass}
          placeholder="Repository URL"
          aria-label="Repository URL"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          required
        />
      )}
      <div className="flex gap-1">
        <input
          className={inputClass}
          placeholder={machine === null ? "C:\\dev" : "/home/me/dev"}
          aria-label={folderLabel}
          value={folder}
          onChange={(e) => setFolder(e.target.value)}
          required
        />
        <Button type="button" size="sm" variant="outline" onClick={browse} aria-expanded={machine === null ? undefined : browsing}>
          Browse...
        </Button>
      </div>
      {browsing && machine !== null && (
        <RemoteFolderBrowser
          machine={machine}
          initialPath={folder}
          onSelect={(picked) => {
            setFolder(picked);
            setBrowsing(false);
          }}
          onCancel={() => setBrowsing(false)}
        />
      )}
      {pickError && (
        <p role="alert" className="text-xs text-destructive">
          {pickError}
        </p>
      )}
      <input
        className={inputClass}
        placeholder="Name"
        aria-label="Name"
        value={effectiveName}
        onChange={(e) => setName(e.target.value)}
        required
      />
      {cloning && path && <p className="truncate text-xs text-muted-foreground" title={path}>Clones into {path}</p>}
      <input
        className={inputClass}
        placeholder="Base branch (detected if empty)"
        aria-label="Base branch"
        value={baseBranch}
        onChange={(e) => setBaseBranch(e.target.value)}
      />
      {createWorkspace.isError && (
        <p role="alert" className="text-xs text-destructive">
          {createWorkspace.error.message}
        </p>
      )}
      <Button type="submit" size="sm" disabled={createWorkspace.isPending}>
        {createWorkspace.isPending
          ? cloning
            ? "Cloning..."
            : "Opening..."
          : cloning
            ? "Clone workspace"
            : "Open workspace"}
      </Button>
    </form>
  );
}
