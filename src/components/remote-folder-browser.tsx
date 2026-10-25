import { useEffect, useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";
import type { Machine } from "@/lib/bindings";
import { useDirectories } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function joinPosix(base: string, name: string): string {
  return base.endsWith("/") ? `${base}${name}` : `${base}/${name}`;
}

function parentOf(path: string): string {
  return path.replace(/\/[^/]+\/?$/, "") || "/";
}

type RemoteFolderBrowserProps = {
  /** An SSH machine, or `local` for the server. */
  machine: Machine;
  /** Directory to start in; the home directory when empty. */
  initialPath: string;
  onSelect: (path: string) => void;
  onCancel: () => void;
};

export function RemoteFolderBrowser({ machine, initialPath, onSelect, onCancel }: RemoteFolderBrowserProps) {
  const [path, setPath] = useState<string | null>(initialPath.trim() || null);
  const [draft, setDraft] = useState(initialPath);
  const listing = useDirectories(machine, path);
  const current = listing.data?.path;
  const machineName = machine.kind === "local" ? "the server" : machine.label;

  useEffect(() => {
    if (current) setDraft(current);
  }, [current]);

  function go() {
    setPath(draft.trim() || null);
  }

  // The browser sits inside the workspace form, so Enter must not submit that form; a
  // nested <form> is invalid HTML and browsers submit it natively instead.
  function goOnEnter(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    go();
  }

  return (
    <div role="group" aria-label={`Browse folders on ${machineName}`} className="flex flex-col gap-1.5 rounded-lg border bg-background p-1.5">
      <div className="flex gap-1">
        <input
          className="h-7 min-w-0 flex-1 rounded-md border border-input bg-background px-2 font-mono text-xs outline-none focus-visible:border-ring"
          aria-label="Remote folder"
          placeholder="~"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={goOnEnter}
        />
        <Button type="button" size="xs" variant="outline" onClick={go}>
          Go
        </Button>
      </div>
      {listing.isPending && <p className="px-1 text-xs text-muted-foreground">{machine.kind === "local" ? "Reading folders..." : `Connecting to ${machineName}...`}</p>}
      {listing.isError && (
        <p role="alert" className="px-1 text-xs break-words text-destructive">
          {listing.error.message}
        </p>
      )}
      {listing.data && current && (
        <ul aria-label={`Folders in ${current}`} className="flex max-h-48 flex-col overflow-y-auto font-mono text-xs">
          {current !== "/" && (
            <li>
              <FolderButton label=".." onClick={() => setPath(parentOf(current))} />
            </li>
          )}
          {listing.data.directories.map((name) => (
            <li key={name}>
              <FolderButton label={`${name}/`} onClick={() => setPath(joinPosix(current, name))} />
            </li>
          ))}
          {listing.data.directories.length === 0 && (
            <li className="px-1.5 py-0.5 text-muted-foreground">No subfolders</li>
          )}
        </ul>
      )}
      <div className="flex justify-end gap-1">
        <Button type="button" size="xs" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button type="button" size="xs" disabled={!current} onClick={() => current && onSelect(current)}>
          Select folder
        </Button>
      </div>
    </div>
  );
}

function FolderButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn("w-full truncate rounded px-1.5 py-0.5 text-left hover:bg-sidebar-accent")}
    >
      {label}
    </button>
  );
}
