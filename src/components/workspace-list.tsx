import { useState, useSyncExternalStore, type FormEvent, type KeyboardEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Laptop, Plus, Server, X } from "lucide-react";
import { RemoteFolderBrowser, joinPosix } from "@/components/remote-folder-browser";
import { Button } from "@/components/ui/button";
import type { HerdrMachine, Workspace, WorkspaceId, WorkspaceLocation, WorkspaceSource } from "@/lib/bindings";
import { runsOnServer } from "@/lib/platform";
import {
  useAddHerdrMachine,
  useAddSshHost,
  useCreateWorkspace,
  useDeleteWorkspace,
  useHerdrMachines,
  useSshHosts,
  useWorkspaces,
} from "@/lib/queries";
import { cn } from "@/lib/utils";
import { reachability, subscribeReachability, unreachableCause } from "@/web/connection";

export const inputClass =
  "h-8 w-full min-w-0 rounded-lg border border-input bg-background px-2.5 text-sm outline-none max-md:h-11 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

/** A small select for toolbars and inline pickers. */
export const compactSelectClass =
  "h-6 rounded-md border border-input bg-background px-1.5 text-xs outline-none max-md:h-11 max-md:px-2.5 max-md:text-sm focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50";

export function describeLocation(location: WorkspaceLocation): string {
  return location.kind === "local" ? location.path : `${location.machine}:${location.path}`;
}

/** SSH host of a herdr machine, or `null` for this computer. */
type MachineKey = string | null;

type Mode = WorkspaceSource["kind"];

/**
 * How each way of adding a workspace on a machine is offered and submitted. The buttons are
 * text only, so the three of them leave room for the machine name in the narrow sidebar.
 */
const modes: Record<Mode, { label: string; title: string; submit: string; pending: string }> = {
  clone: { label: "Clone", title: "Clone a repository", submit: "Clone workspace", pending: "Cloning..." },
  open: { label: "Open", title: "Open a cloned repository", submit: "Open workspace", pending: "Opening..." },
  init: {
    label: "Init",
    title: "Make a folder a new git repository",
    submit: "Init workspace",
    pending: "Initializing...",
  },
};

function machineKeyOf(location: WorkspaceLocation): MachineKey {
  return location.kind === "local" ? null : location.machine;
}

/** Why herdr cannot run commands on `host`, or `null` when it can. */
function herdrProblem(host: string, machines: HerdrMachine[]): string | null {
  const saved = machines.filter((machine) => machine.target === host);
  if (saved.length === 0) return "Not saved in herdr";
  return saved.some((machine) => machine.enabled) ? null : "Disabled in herdr";
}

function joinLocal(base: string, name: string): string {
  if (runsOnServer) return joinPosix(base, name);
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
  const reach = useSyncExternalStore(subscribeReachability, reachability);
  const serverDown = reach === "unreachable" || reach === "retrying";
  const herdrMachines = useHerdrMachines();
  const [adding, setAdding] = useState<{ machine: MachineKey; mode: Mode } | null>(null);
  const [addingMachine, setAddingMachine] = useState(false);

  const saved = herdrMachines.data ?? [];
  const remotes = [
    ...new Set([
      ...saved.map((machine) => machine.target),
      ...(workspaces.data ?? []).flatMap((w) => (w.location.kind === "ssh" ? [w.location.machine] : [])),
    ]),
  ];
  const machines: MachineKey[] = [null, ...remotes];

  return (
    <section aria-labelledby="workspaces-heading" className="flex shrink-0 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 id="workspaces-heading" className="text-xs font-medium uppercase text-muted-foreground">
          Workspaces
        </h2>
        <Button variant="ghost" size="xs" onClick={() => setAddingMachine((open) => !open)}>
          {!addingMachine && <Plus />}
          {addingMachine ? "Cancel" : "Add machine"}
        </Button>
      </div>

      {addingMachine && (
        <AddMachineForm
          saved={saved.map((machine) => machine.target)}
          onAdded={() => setAddingMachine(false)}
          onCancel={() => setAddingMachine(false)}
        />
      )}
      {herdrMachines.isError && (
        <p role="alert" className="text-xs text-destructive">
          Failed to list herdr machines: {herdrMachines.error.message}
        </p>
      )}
      {workspaces.isPending && !serverDown && (
        <p className="text-sm text-muted-foreground">Loading workspaces...</p>
      )}
      {(workspaces.isError || (workspaces.isPending && serverDown)) && (
        <div role="alert" className="flex flex-col items-start gap-2 text-sm text-destructive">
          <p>
            Failed to load workspaces:{" "}
            {workspaces.error?.message ?? (unreachableCause() || "The Toodo server does not answer.")}
          </p>
          <Button size="xs" variant="outline" onClick={() => void workspaces.refetch()}>
            Retry now
          </Button>
        </div>
      )}
      {workspaces.data?.length === 0 && adding === null && (
        <p className="text-sm text-muted-foreground">No workspaces yet.</p>
      )}

      <div className="flex flex-col gap-3">
        {workspaces.data &&
          machines.map((machine) => {
            const label = machine ?? "Local";
            const items = workspaces.data.filter((w) => machineKeyOf(w.location) === machine);
            const active = adding?.machine === machine ? adding.mode : null;
            const problem = machine !== null && herdrMachines.isSuccess ? herdrProblem(machine, saved) : null;
            return (
              <section key={label} aria-label={`Machine ${label}`} className="flex flex-col gap-1">
                <div className="flex items-center gap-1">
                  <h3 className="flex min-w-0 flex-1 items-center gap-1.5 px-1 text-xs font-semibold">
                    {machine === null ? (
                      <Laptop aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
                    ) : (
                      <Server aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
                    )}
                    <span className="truncate">{label}</span>
                    {problem && <span className="ml-1.5 font-normal text-destructive">{problem}</span>}
                  </h3>
                  {(["clone", "open", "init"] as const).map((mode) => (
                    <Button
                      key={mode}
                      size="xs"
                      variant={active === mode ? "secondary" : "outline"}
                      aria-label={`${modes[mode].label} on ${label}`}
                      aria-pressed={active === mode}
                      title={modes[mode].title}
                      onClick={() => setAdding(active === mode ? null : { machine, mode })}
                    >
                      {modes[mode].label}
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
                    onCancel={() => setAdding(null)}
                  />
                )}
                {items.length > 0 && (
                  <ul className="flex flex-col gap-1">
                    {items.map((workspace) => (
                      <WorkspaceItem
                        key={workspace.id}
                        workspace={workspace}
                        selected={workspace.id === selectedId}
                        onSelect={onSelect}
                      />
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

type WorkspaceItemProps = {
  workspace: Workspace;
  selected: boolean;
  onSelect: (id: WorkspaceId) => void;
};

/** A workspace in the list; removing it asks for confirmation first. */
function WorkspaceItem({ workspace, selected, onSelect }: WorkspaceItemProps) {
  const deleteWorkspace = useDeleteWorkspace();
  const [confirming, setConfirming] = useState(false);

  return (
    <li className="flex flex-col gap-1">
      <div
        className={cn(
          "group flex items-center rounded-lg hover:bg-sidebar-accent",
          selected && "bg-sidebar-accent",
        )}
      >
        <button
          type="button"
          aria-current={selected ? "true" : undefined}
          onClick={() => onSelect(workspace.id)}
          className="flex min-w-0 flex-1 flex-col px-2.5 py-1.5 text-left"
        >
          <span className="truncate text-sm font-medium">{workspace.name}</span>
          <span className="truncate text-xs text-muted-foreground">{workspace.location.path}</span>
        </button>
        {!confirming && (
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label={`Remove ${workspace.name}`}
            title="Remove the workspace and its tasks from Toodo"
            className="mr-1 shrink-0 opacity-0 group-hover:opacity-100 hover:text-destructive focus-visible:opacity-100 [@media(hover:none)]:opacity-100"
            onClick={() => {
              deleteWorkspace.reset();
              setConfirming(true);
            }}
          >
            <X />
          </Button>
        )}
      </div>
      {confirming && (
        <div
          role="group"
          aria-label={`Remove ${workspace.name}`}
          className="flex flex-col gap-1.5 rounded-lg border p-2"
          onKeyDown={(event) => {
            if (event.key === "Escape" && !deleteWorkspace.isPending) {
              event.preventDefault();
              setConfirming(false);
            }
          }}
        >
          <p className="text-xs text-muted-foreground">
            Remove {workspace.name} and its tasks from Toodo? The repository, worktrees, and herdr workspaces are
            kept.
          </p>
          {deleteWorkspace.isError && (
            <p role="alert" className="text-xs text-destructive">
              {deleteWorkspace.error.message}
            </p>
          )}
          <div className="flex gap-1">
            <Button
              changesData
              size="xs"
              variant="destructive"
              aria-label={`Confirm remove ${workspace.name}`}
              disabled={deleteWorkspace.isPending}
              onClick={() => deleteWorkspace.mutate(workspace.id)}
            >
              {deleteWorkspace.isPending ? "Removing..." : "Remove"}
            </Button>
            <Button size="xs" variant="ghost" autoFocus onClick={() => setConfirming(false)}>
              Back
            </Button>
          </div>
        </div>
      )}
    </li>
  );
}

/** Calls `close` when Escape is pressed inside an inline form, unless it is busy. */
export function escapeCloses(close: () => void, busy = false) {
  return (event: KeyboardEvent) => {
    if (event.key === "Escape" && !busy && !event.defaultPrevented) {
      event.preventDefault();
      close();
    }
  };
}

function AddMachineForm({ saved, onAdded, onCancel }: { saved: string[]; onAdded: () => void; onCancel: () => void }) {
  const sshHosts = useSshHosts();
  const addMachine = useAddHerdrMachine();
  const [host, setHost] = useState("");
  const [addingHost, setAddingHost] = useState(false);

  const candidates = [...new Set([...(sshHosts.data ?? []), ...(host ? [host] : [])])].filter(
    (alias) => !saved.includes(alias),
  );

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    addMachine.mutate(host, { onSuccess: onAdded });
  }

  return (
    <div className="flex flex-col gap-2" onKeyDown={escapeCloses(onCancel, addMachine.isPending)}>
      <form onSubmit={submit} aria-label="Add machine" className="flex flex-col gap-2 rounded-lg border p-2">
        <p className="text-xs text-muted-foreground">
          Runs herdr machine add in a terminal window. Answer its prompts there; herdr offers to install itself on
          the host when it is missing.
        </p>
        <div className="flex gap-1">
          <select
            className={inputClass}
            aria-label="SSH host"
            autoFocus
            value={host}
            onChange={(e) => setHost(e.target.value)}
            required
          >
            <option value="">SSH host...</option>
            {candidates.map((alias) => (
              <option key={alias} value={alias}>
                {alias}
              </option>
            ))}
          </select>
          <Button
            type="button"
            size="sm"
            variant="outline"
            aria-expanded={addingHost}
            onClick={() => setAddingHost((open) => !open)}
          >
            New host
          </Button>
        </div>
        {sshHosts.isError && (
          <p role="alert" className="text-xs text-destructive">
            Failed to read the SSH config: {sshHosts.error.message}
          </p>
        )}
        {addMachine.isError && (
          <p role="alert" className="text-xs text-destructive">
            {addMachine.error.message}
          </p>
        )}
        <Button type="submit" size="sm" disabled={addMachine.isPending}>
          {addMachine.isPending ? "Waiting for the herdr terminal..." : "Add to herdr"}
        </Button>
      </form>
      {addingHost && (
        <NewSshHostForm
          onAdded={(alias) => {
            setHost(alias);
            setAddingHost(false);
          }}
          onCancel={() => setAddingHost(false)}
        />
      )}
    </div>
  );
}

function NewSshHostForm({ onAdded, onCancel }: { onAdded: (alias: string) => void; onCancel: () => void }) {
  const addHost = useAddSshHost();
  const [alias, setAlias] = useState("");
  const [hostName, setHostName] = useState("");
  const [user, setUser] = useState("");
  const [port, setPort] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    addHost.mutate(
      { alias, hostName, user: user.trim() || null, port: port.trim() ? Number(port) : null },
      { onSuccess: () => onAdded(alias.trim()) },
    );
  }

  return (
    <form
      onSubmit={submit}
      aria-label="New SSH host"
      className="flex flex-col gap-2 rounded-lg border p-2"
      onKeyDown={escapeCloses(onCancel, addHost.isPending)}
    >
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
  onCancel: () => void;
};

function NewWorkspaceForm({ machine, mode, onCreated, onCancel }: NewWorkspaceFormProps) {
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

  // The desktop app picks local folders with the system dialog; everything else, the
  // server's own folders included, is browsed in the app.
  const browsesInApp = machine !== null || runsOnServer;

  async function browse() {
    if (browsesInApp) {
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
    const source: WorkspaceSource = mode === "clone" ? { kind: "clone", url } : { kind: mode };
    createWorkspace.mutate(
      { name: effectiveName, location, source, defaultBaseBranch: baseBranch.trim() || null },
      { onSuccess: onCreated },
    );
  }

  return (
    <form
      onSubmit={submit}
      aria-label={`${modes[mode].label} workspace on ${machine ?? "Local"}`}
      className="flex flex-col gap-2 rounded-lg border p-2"
      onKeyDown={escapeCloses(onCancel, createWorkspace.isPending)}
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
          placeholder={machine === null && !runsOnServer ? "C:\\dev" : "/home/me/dev"}
          aria-label={folderLabel}
          value={folder}
          onChange={(e) => setFolder(e.target.value)}
          required
        />
        <Button type="button" size="sm" variant="outline" onClick={browse} aria-expanded={browsesInApp ? browsing : undefined}>
          Browse...
        </Button>
      </div>
      {browsing && browsesInApp && (
        <RemoteFolderBrowser
          machine={machine === null ? { kind: "local" } : { kind: "ssh", label: machine }}
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
      {mode === "init" && (
        <p className="text-xs text-muted-foreground">
          The folder is created if it does not exist and becomes a git repository without a remote.
        </p>
      )}
      <input
        className={inputClass}
        placeholder={mode === "init" ? "Base branch (main if empty)" : "Base branch (detected if empty)"}
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
        {createWorkspace.isPending ? modes[mode].pending : modes[mode].submit}
      </Button>
    </form>
  );
}
