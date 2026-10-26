/**
 * The live view of the panes of a started task's herdr workspace, mirrored by
 * `toodo-server`. It opens read-only; typing reaches the pane only after "Take control".
 */
import { useEffect, useRef, useState } from "react";
import { Expand, Keyboard, RotateCw, Shrink } from "lucide-react";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Button } from "@/components/ui/button";
import type { Task, TaskPane, Workspace } from "@/lib/bindings";
import { useTaskPanes, useWorkspaces } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { openPaneStream, type PaneClientMessage, type PaneServerMessage, type PaneSocket } from "@/web/pane-stream";

type StreamState =
  | { kind: "connecting" }
  | { kind: "live" }
  | { kind: "closed"; reason: string };

/** The width of a terminal cell relative to the font size, for fitting frames. */
const CELL_WIDTH = 0.6;
const MIN_FONT_SIZE = 6;
const MAX_FONT_SIZE = 13;

function paneName(pane: TaskPane): string {
  return pane.title ?? pane.agent ?? pane.paneId;
}

function defaultPane(panes: TaskPane[]): string | null {
  return (panes.find((pane) => pane.agent !== null) ?? panes[0])?.paneId ?? null;
}

/**
 * The Windows machine the task's workspace is on, if it is on one: herdr there serves its
 * API on a named pipe that the server cannot reach over SSH, so it has no pane view.
 */
function windowsMachine(workspace: Workspace | undefined): string | null {
  const location = workspace?.location;
  return location?.kind === "ssh" && !location.path.startsWith("/") ? location.machine : null;
}

export default function PaneView({ task }: { task: Task }) {
  const panes = useTaskPanes(task.id);
  const workspaces = useWorkspaces();
  const windows = windowsMachine(workspaces.data?.find((workspace) => workspace.id === task.workspaceId));
  const [chosen, setChosen] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const list = panes.data ?? [];
  const paneId = list.some((pane) => pane.paneId === chosen) ? chosen : defaultPane(list);

  return (
    <section
      aria-label={`Panes of #${task.id}`}
      className={cn(
        "flex min-w-0 flex-col gap-1.5",
        expanded && "fixed inset-4 z-50 rounded-lg border bg-background p-3 shadow-lg",
      )}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-1">
        <h4 className="mr-auto text-xs font-medium text-muted-foreground">Panes</h4>
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={expanded ? "Shrink pane view" : "Expand pane view"}
          aria-pressed={expanded}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? <Shrink /> : <Expand />}
        </Button>
      </div>
      {panes.error && (
        <p role="alert" className="text-xs text-destructive">
          {panes.error.message}
        </p>
      )}
      {panes.isPending && <p className="text-xs text-muted-foreground">Opening the herdr workspace...</p>}
      {panes.data && list.length === 0 && <p className="text-xs text-muted-foreground">The workspace has no panes.</p>}
      {list.length > 1 && (
        <div role="group" aria-label={`Pane of #${task.id}`} className="flex flex-wrap gap-1">
          {list.map((pane) => (
            <Button
              key={pane.paneId}
              size="xs"
              variant={pane.paneId === paneId ? "secondary" : "ghost"}
              aria-pressed={pane.paneId === paneId}
              onClick={() => setChosen(pane.paneId)}
            >
              <span className="max-w-40 truncate">{paneName(pane)}</span>
            </Button>
          ))}
        </div>
      )}
      {windows !== null ? (
        <p role="note" className="text-xs text-muted-foreground">
          The pane view is not available for the Windows machine {windows} yet. Open the pane in herdr there.
        </p>
      ) : (
        paneId !== null &&
        !workspaces.isPending && <PaneTerminal key={paneId} taskId={task.id} paneId={paneId} expanded={expanded} />
      )}
    </section>
  );
}

function PaneTerminal({ taskId, paneId, expanded }: { taskId: number; paneId: string; expanded: boolean }) {
  const container = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const socket = useRef<PaneSocket | null>(null);
  const size = useRef({ cols: 80, rows: 24 });
  const controlling = useRef(false);
  const [state, setState] = useState<StreamState>({ kind: "connecting" });
  const [control, setControl] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  function fit() {
    const term = terminal.current;
    const width = container.current?.clientWidth ?? 0;
    if (!term || width === 0) {
      return;
    }
    const fontSize = Math.floor(width / (size.current.cols * CELL_WIDTH));
    term.options.fontSize = Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, fontSize));
  }

  useEffect(() => {
    const element = container.current;
    if (!element) {
      return;
    }
    const term = new Terminal({
      disableStdin: true,
      cursorBlink: false,
      scrollback: 0,
      convertEol: false,
      fontSize: MAX_FONT_SIZE,
    });
    terminal.current = term;
    term.open(element);
    const typed = term.onData((data) => {
      if (controlling.current) {
        send({ type: "text", text: data });
      }
    });
    const observer = new ResizeObserver(() => fit());
    observer.observe(element);
    return () => {
      observer.disconnect();
      typed.dispose();
      term.dispose();
      terminal.current = null;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    setState({ kind: "connecting" });
    setControl(false);
    setNotice(null);
    openPaneStream(taskId, paneId).then(
      (opened) => {
        if (disposed) {
          opened.close();
          return;
        }
        socket.current = opened;
        opened.onopen = () => setState({ kind: "live" });
        opened.onmessage = (event: MessageEvent<string>) => receive(JSON.parse(event.data) as PaneServerMessage);
        opened.onclose = () => {
          setControl(false);
          setState((current) => (current.kind === "closed" ? current : { kind: "closed", reason: "disconnected" }));
        };
      },
      (error: unknown) => {
        if (!disposed) {
          setState({ kind: "closed", reason: error instanceof Error ? error.message : String(error) });
        }
      },
    );
    return () => {
      disposed = true;
      socket.current?.close();
      socket.current = null;
    };
  }, [taskId, paneId, attempt]);

  useEffect(() => {
    controlling.current = control;
    const term = terminal.current;
    if (term) {
      term.options.disableStdin = !control;
      if (control) {
        term.focus();
      }
    }
  }, [control]);

  useEffect(() => fit(), [expanded]);

  function receive(message: PaneServerMessage) {
    const term = terminal.current;
    switch (message.type) {
      case "frame":
        if (!term) {
          return;
        }
        if (message.cols !== size.current.cols || message.rows !== size.current.rows) {
          size.current = { cols: message.cols, rows: message.rows };
          term.resize(message.cols, message.rows);
          fit();
        }
        // Each frame is the whole screen: draw it from a clean, cursorless terminal.
        term.reset();
        term.write(`\x1b[?25l${message.ansi}`);
        return;
      case "control":
        setControl(message.on);
        return;
      case "error":
        setNotice(message.message);
        return;
      case "closed":
        setState({ kind: "closed", reason: message.reason === "pane_closed" ? "The pane was closed." : message.reason });
        return;
    }
  }

  function send(message: PaneClientMessage) {
    const open = socket.current;
    if (open && open.readyState === WebSocket.OPEN) {
      open.send(JSON.stringify(message));
    }
  }

  const live = state.kind === "live";
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-1">
        <span
          role="status"
          aria-label={`Pane stream of #${taskId}`}
          className={cn("mr-auto text-xs", state.kind === "closed" ? "text-destructive" : "text-muted-foreground")}
        >
          {state.kind === "connecting" ? "Connecting..." : state.kind === "live" ? (control ? "Live, in control" : "Live, read-only") : state.reason}
        </span>
        {state.kind === "closed" && (
          <Button size="xs" variant="outline" onClick={() => setAttempt((value) => value + 1)}>
            <RotateCw />
            Reconnect
          </Button>
        )}
        <Button
          size="xs"
          variant={control ? "default" : "outline"}
          aria-pressed={control}
          disabled={!live}
          title={control ? "Stop sending your typing to the pane" : "Send your typing to the pane"}
          onClick={() => send({ type: "control", on: !control })}
        >
          <Keyboard />
          {control ? "Release control" : "Take control"}
        </Button>
      </div>
      {notice && (
        <p role="alert" className="text-xs text-destructive">
          {notice}
        </p>
      )}
      <div
        ref={container}
        data-control={control}
        aria-label={`Pane ${paneId}`}
        className={cn(
          "min-w-0 overflow-auto rounded-md border bg-black p-1",
          expanded && "min-h-0 flex-1",
          control && "ring-2 ring-amber-500",
        )}
      />
    </div>
  );
}
