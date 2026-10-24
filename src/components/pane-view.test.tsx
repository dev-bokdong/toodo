import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PaneView from "@/components/pane-view";
import { commands, type Task, type TaskPane } from "@/lib/bindings";
import { openPaneStream } from "@/web/pane-stream";

const { FakeTerminal, terminals } = vi.hoisted(() => {
  class FakeTerminal {
    options: Record<string, unknown>;
    written: string[] = [];
    dataListener: ((data: string) => void) | null = null;

    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      terminals.push(this);
    }

    open() {}
    focus() {}
    dispose() {}
    reset() {
      this.written = [];
    }
    resize(cols: number, rows: number) {
      this.options.cols = cols;
      this.options.rows = rows;
    }
    write(data: string) {
      this.written.push(data);
    }
    onData(listener: (data: string) => void) {
      this.dataListener = listener;
      return { dispose: () => (this.dataListener = null) };
    }
  }
  const terminals: FakeTerminal[] = [];
  return { FakeTerminal, terminals };
});

vi.mock("@xterm/xterm", () => ({ Terminal: FakeTerminal }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("@/web/pane-stream", () => ({ openPaneStream: vi.fn() }));
vi.mock("@/lib/bindings", () => ({ commands: { ensureTaskWorkspace: vi.fn() } }));

class FakeSocket {
  readyState = WebSocket.OPEN;
  sent: unknown[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  closed = false;

  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    this.closed = true;
  }
  receive(message: unknown) {
    act(() => {
      this.onmessage?.({ data: JSON.stringify(message) });
    });
  }
}

const panes: TaskPane[] = [
  { paneId: "w27:p1", agent: null, agentStatus: null, title: "me@devbox:~" },
  { paneId: "w27:p2", agent: "claude", agentStatus: "working", title: "Add login" },
];

const task = { id: 7, status: "in_progress", herdrWorkspaceId: "w27" } as Task;

function renderView() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={queryClient}>
      <PaneView task={task} />
    </QueryClientProvider>,
  );
}

async function connected(): Promise<FakeSocket> {
  const socket = new FakeSocket();
  vi.mocked(openPaneStream).mockResolvedValueOnce(socket as unknown as WebSocket);
  renderView();
  await waitFor(() => expect(socket.onopen).not.toBeNull());
  act(() => {
    socket.onopen?.();
  });
  return socket;
}

describe("PaneView", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    terminals.length = 0;
    vi.mocked(commands.ensureTaskWorkspace).mockResolvedValue({ status: "ok", data: panes });
  });

  it("shows the agent's pane first and draws each frame", async () => {
    const socket = await connected();
    expect(openPaneStream).toHaveBeenCalledWith(7, "w27:p2");
    socket.receive({ type: "frame", seq: 1, cols: 120, rows: 40, ansi: "\x1b[1mhello" });
    const terminal = terminals[0];
    expect(terminal.options).toMatchObject({ cols: 120, rows: 40 });
    expect(terminal.written.join("")).toContain("\x1b[1mhello");
    expect(screen.getByRole("status", { name: "Pane stream of #7" })).toHaveTextContent("Live, read-only");
    expect(screen.getByRole("button", { name: "Add login" })).toHaveAttribute("aria-pressed", "true");
  });

  it("sends typing only after control was taken", async () => {
    const socket = await connected();
    const terminal = terminals[0];
    terminal.dataListener?.("rm -rf /\r");
    expect(socket.sent).toEqual([]);
    expect(terminal.options.disableStdin).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Take control" }));
    expect(socket.sent).toEqual([{ type: "control", on: true }]);
    // Typing still waits for the server to confirm control.
    terminal.dataListener?.("x");
    expect(socket.sent).toHaveLength(1);

    socket.receive({ type: "control", on: true });
    expect(screen.getByRole("button", { name: "Release control" })).toHaveAttribute("aria-pressed", "true");
    expect(terminal.options.disableStdin).toBe(false);
    terminal.dataListener?.("ls\r");
    expect(socket.sent).toEqual([
      { type: "control", on: true },
      { type: "text", text: "ls\r" },
    ]);
  });

  it("offers to reconnect once the stream closed", async () => {
    const socket = await connected();
    socket.receive({ type: "closed", reason: "pane_closed" });
    expect(screen.getByRole("status", { name: "Pane stream of #7" })).toHaveTextContent("The pane was closed.");

    const next = new FakeSocket();
    vi.mocked(openPaneStream).mockResolvedValueOnce(next as unknown as WebSocket);
    fireEvent.click(screen.getByRole("button", { name: "Reconnect" }));
    await waitFor(() => expect(openPaneStream).toHaveBeenCalledTimes(2));
    expect(socket.closed).toBe(true);
  });
});
