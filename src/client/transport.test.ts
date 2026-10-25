import { beforeEach, describe, expect, it, vi } from "vitest";
import { AccessBlocked, openRemoteSocket, remoteFetch } from "@/client/transport";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const encode = (text: string) => new TextEncoder().encode(text).buffer;

/** Answers `fetch` with `status`, then hands out `chunks` one `read` at a time. */
function answer(status: number, chunks: string[]) {
  invoke.mockImplementation(async (command: string) => {
    if (command === "plugin:remote|fetch") return { id: 7, status, headers: [["content-type", "text/plain"]] };
    if (command === "plugin:remote|read") return encode(chunks.shift() ?? "");
    return null;
  });
}

function commands(): string[] {
  return invoke.mock.calls.map(([command]) => command as string);
}

describe("remoteFetch", () => {
  beforeEach(() => invoke.mockReset());

  it("sends the request through the app and streams the answer's body", async () => {
    answer(200, ["hel", "lo"]);
    const response = await remoteFetch("https://toodo.example/api/rpc/x", {
      method: "POST",
      headers: { Authorization: "Bearer t" },
      body: "{}",
    });

    const [, body, options] = invoke.mock.calls[0] as [string, Uint8Array, { headers: Record<string, string> }];
    expect(new TextDecoder().decode(body)).toBe("{}");
    const sent = JSON.parse(options.headers["toodo-request"]) as { url: string; method: string; headers: string[][] };
    expect(sent.url).toBe("https://toodo.example/api/rpc/x");
    expect(sent.method).toBe("POST");
    expect(sent.headers).toContainEqual(["authorization", "Bearer t"]);
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("text/plain");
    expect(await response.text()).toBe("hello");
  });

  it("turns the app's failures into the errors fetch would throw", async () => {
    invoke.mockRejectedValueOnce({ kind: "network", message: "connection refused" });
    await expect(remoteFetch("https://toodo.example/api")).rejects.toBeInstanceOf(TypeError);
    invoke.mockRejectedValueOnce({ kind: "access", message: "Cloudflare Access said no" });
    await expect(remoteFetch("https://toodo.example/api")).rejects.toEqual(new AccessBlocked("Cloudflare Access said no"));
  });

  it("cancels the body in the app when the request is aborted", async () => {
    answer(200, ["a"]);
    const controller = new AbortController();
    await remoteFetch("https://toodo.example/api/events", { signal: controller.signal });
    controller.abort();
    expect(invoke).toHaveBeenLastCalledWith("plugin:remote|cancel", { id: 7 });
  });

  it("gives answers without a body none", async () => {
    answer(204, []);
    const response = await remoteFetch("https://toodo.example/api/push/subscription", { method: "DELETE" });
    expect(response.body).toBeNull();
    expect(commands()).toEqual(["plugin:remote|fetch", "plugin:remote|cancel"]);
  });
});

describe("openRemoteSocket", () => {
  beforeEach(() => invoke.mockReset());

  it("delivers the socket's messages after its handlers are set, then closes", async () => {
    const messages = ['{"type":"control","on":true}'];
    invoke.mockImplementation(async (command: string) => {
      if (command === "plugin:remote|socket_open") return 3;
      if (command === "plugin:remote|socket_receive") return messages.shift() ?? null;
      return null;
    });
    const socket = await openRemoteSocket("https://toodo.example/api/panes/stream?ticket=t");
    const events: string[] = [];
    socket.onopen = () => events.push("open");
    socket.onmessage = (event) => events.push(event.data);
    const closed = new Promise<void>((resolve) => {
      socket.onclose = () => resolve();
    });

    await closed;
    expect(events).toEqual(["open", '{"type":"control","on":true}']);
    expect(socket.readyState).toBe(WebSocket.CLOSED);
    socket.send("ignored once closed");
    expect(commands()).not.toContain("plugin:remote|socket_send");
  });
});
