import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  invoke: vi.fn(async (_command: string, ..._rest: unknown[]) => "from the app"),
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
  };
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

  it("go to the connected server with its token, except the app's own", async () => {
    const fetch = reply(200, '[{"id":1}]');
    vi.stubGlobal("fetch", fetch);
    const { core, session } = await startApp(A);
    session.setToken("device-a");

    await expect(core.invoke("list_tasks", { workspaceId: 1 })).resolves.toEqual([{ id: 1 }]);
    const [url, init] = fetch.mock.calls[0];
    expect(url).toBe(`${A}/api/rpc/list_tasks`);
    expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer device-a");

    await core.invoke("set_connection", { server: null });
    await core.invoke("plugin:notification|notify", { options: {} });
    expect(native.invoke.mock.calls.map(([command]) => command)).toEqual([
      "set_connection",
      "plugin:notification|notify",
    ]);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("follow the server's event stream, and the app's own events in the app", async () => {
    const fetch = vi.fn(async () => new Response(new ReadableStream(), { status: 200 }));
    vi.stubGlobal("fetch", fetch);
    const { event, session } = await startApp(A);
    session.setToken("device-a");

    const unlisten = await event.listen("tasks-changed", () => {});
    await event.listen("update-check-requested", () => {});
    await event.listen("tauri://focus", () => {});

    await vi.waitFor(() => expect(fetch).toHaveBeenCalled());
    expect(fetch.mock.calls[0]).toEqual([`${A}/api/events`, expect.anything()]);
    expect(native.listen.mock.calls.map(([name]) => name)).toEqual(["update-check-requested", "tauri://focus"]);
    unlisten();
  });
});

describe("what the app keeps for each server", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
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
    vi.stubGlobal("fetch", reply(401, "missing or wrong Toodo access token"));

    await expect(a.core.invoke("list_tasks")).rejects.toThrow("did not accept");

    expect(a.session.getToken()).toBeNull();
    expect((await startApp(B)).session.getToken()).toBe("device-b");
  });
});
