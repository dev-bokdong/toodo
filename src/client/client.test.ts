import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  invoke: vi.fn(async (_command: string, ..._rest: unknown[]): Promise<unknown> => "from the app"),
  listen: vi.fn(async (_event: string, ..._rest: unknown[]) => () => {}),
  once: vi.fn(async () => () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: native.listen, once: native.once }));

const A = "https://a.example";
const B = "http://192.168.0.5:47616";

/** Loads the modules afresh as the desktop app started with `server`, as after a restart. */
async function startApp(server: string | null) {
  vi.resetModules();
  window.__TOODO_SERVER__ = server;
  return {
    core: await import("@/client/core"),
    event: await import("@/client/event"),
    session: await import("@/web/session"),
    chat: await import("@/lib/chat"),
    connection: await import("@/web/connection"),
  };
}

type Sent = { url: string; method: string; headers: [string, string][] };

/**
 * Stands in for the app's `remote` plugin (`src-tauri/src/remote/`), which sends the
 * requests to the server: it answers each with `status` and `body`, or with a body that
 * never ends for `null`. Returns the requests sent.
 */
function fakeRemote(status: number, body: string | null): Sent[] {
  const sent: Sent[] = [];
  native.invoke.mockImplementation(async (command: string, ...rest: unknown[]) => {
    if (command === "plugin:remote|fetch") {
      const options = rest[1] as { headers: Record<string, string> };
      sent.push(JSON.parse(options.headers["toodo-request"]) as Sent);
      return { id: sent.length, status, headers: [] };
    }
    if (command === "plugin:remote|read") {
      if (body === null) return new Promise(() => {});
      const chunk = new TextEncoder().encode(body).buffer;
      body = "";
      return chunk;
    }
    return "from the app";
  });
  return sent;
}

function appCommands(): string[] {
  return native.invoke.mock.calls.map(([command]) => command).filter((command) => !command.startsWith("plugin:remote|"));
}

function header(request: Sent, name: string): string | undefined {
  return request.headers.find(([key]) => key === name.toLowerCase())?.[1];
}

function reply(status: number, body: string) {
  return vi.fn(async (_url: string, _init?: RequestInit) => new Response(body, { status }));
}

describe("the desktop app's commands and events", () => {
  beforeEach(() => {
    native.invoke.mockClear();
    native.listen.mockClear();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    native.invoke.mockImplementation(async () => "from the app");
    localStorage.clear();
    delete window.__TOODO_SERVER__;
  });

  it("run in the app when it shows its own data", async () => {
    const fetch = reply(200, "[]");
    vi.stubGlobal("fetch", fetch);
    const { core, event } = await startApp(null);

    await expect(core.invoke("list_tasks", { workspaceId: 1 })).resolves.toBe("from the app");
    await event.listen("tasks-changed", () => {});

    expect(native.invoke).toHaveBeenCalledWith("list_tasks", { workspaceId: 1 }, undefined);
    expect(native.listen).toHaveBeenCalledWith("tasks-changed", expect.any(Function), undefined);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("go to the connected server from the app with its token, except the app's own", async () => {
    const fetch = reply(200, "[]");
    vi.stubGlobal("fetch", fetch);
    const sent = fakeRemote(200, '[{"id":1}]');
    const { core, session } = await startApp(A);
    session.setToken("device-a");

    await expect(core.invoke("list_tasks", { workspaceId: 1 })).resolves.toEqual([{ id: 1 }]);
    expect(sent[0].url).toBe(`${A}/api/rpc/list_tasks`);
    expect(sent[0].method).toBe("POST");
    expect(header(sent[0], "Authorization")).toBe("Bearer device-a");

    await core.invoke("set_connection", { server: null });
    await core.invoke("set_access_token", { server: A, token: null });
    await core.invoke("plugin:notification|notify", { options: {} });
    expect(appCommands()).toEqual(["set_connection", "set_access_token", "plugin:notification|notify"]);
    expect(sent).toHaveLength(1);
    expect(fetch).not.toHaveBeenCalled();
  });

  it("follow the server's event stream, and the app's own events in the app", async () => {
    const sent = fakeRemote(200, null);
    const { event, session } = await startApp(A);
    session.setToken("device-a");

    const unlisten = await event.listen("tasks-changed", () => {});
    await event.listen("update-check-requested", () => {});
    await event.listen("tauri://focus", () => {});

    await vi.waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].url).toBe(`${A}/api/events`);
    expect(native.listen.mock.calls.map(([name]) => name)).toEqual(["update-check-requested", "tauri://focus"]);
    unlisten();
  });

  it("keep the token when Cloudflare Access blocks them, and tell why", async () => {
    const why = `Cloudflare Access protects ${A} and wants a sign-in, which the app cannot do.`;
    native.invoke.mockImplementation(async (command: string) => {
      if (command.startsWith("plugin:remote|")) throw { kind: "access", message: why };
      return "from the app";
    });
    const { core, session, connection } = await startApp(A);
    session.setToken("device-a");

    await expect(core.invoke("list_tasks")).rejects.toThrow(why);

    expect(session.getToken()).toBe("device-a");
    await vi.waitFor(() => expect(connection.reachability()).toBe("unreachable"));
    expect(connection.unreachableCause()).toBe(why);
  });
});

describe("what the app keeps for each server", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    native.invoke.mockImplementation(async () => "from the app");
    localStorage.clear();
    delete window.__TOODO_SERVER__;
  });

  it("keeps the token and the chat's read mark apart", async () => {
    const a = await startApp(A);
    a.session.setToken("device-a");
    a.chat.markRead(7);

    const b = await startApp(B);
    expect(b.session.getToken()).toBeNull();
    b.session.setToken("device-b");
    b.chat.markRead(2);

    const web = await startApp(null);
    expect(web.session.getToken()).toBeNull();

    const again = await startApp(A);
    expect(again.session.getToken()).toBe("device-a");
    expect(localStorage.getItem(`toodo-chat-read@${A}`)).toBe("7");
    expect(localStorage.getItem(`toodo-chat-read@${B}`)).toBe("2");
  });

  it("forgets only the token of the server that revoked it", async () => {
    const b = await startApp(B);
    b.session.setToken("device-b");
    const a = await startApp(A);
    a.session.setToken("device-a");
    fakeRemote(401, "missing or wrong Toodo access token");

    await expect(a.core.invoke("list_tasks")).rejects.toThrow("did not accept");

    expect(a.session.getToken()).toBeNull();
    expect((await startApp(B)).session.getToken()).toBe("device-b");
  });
});
