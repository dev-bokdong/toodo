import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const native = vi.hoisted(() => ({
  invoke: vi.fn(async (_command: string, _args?: unknown) => null as unknown),
  relaunch: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), once: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: native.relaunch }));

const A = "https://a.example";
const B = "http://192.168.0.5:47616";
const saved = { server: A, servers: [{ name: "Home", url: A }, { name: "Work", url: B }] };

/** Loads the modules afresh as a window started with `server`, as after a reload or restart. */
async function startWindow(server: string | null) {
  vi.resetModules();
  window.__TOODO_SERVER__ = server;
  return {
    servers: await import("@/lib/servers"),
    session: await import("@/web/session"),
    event: await import("@/client/event"),
    connection: await import("@/web/connection"),
  };
}

type Sent = { url: string; method: string; headers: [string, string][] };
type Answer = { status: number; body: string | null } | "network";

/**
 * Stands in for the app's `remote` plugin (`src-tauri/src/remote/`), which sends the
 * window's requests to the server: `answer` gives each one's status and body (a body that
 * never ends for `null`) or a network failure.
 */
let answer: (request: Sent) => Answer = () => ({ status: 200, body: "" });
const bodies = new Map<number, string | null>();

function remote(command: string, rest: unknown[]): unknown {
  if (command === "plugin:remote|fetch") {
    const options = rest[1] as { headers: Record<string, string> };
    const request = JSON.parse(options.headers["toodo-request"]) as Sent;
    const answered = answer(request);
    if (answered === "network") throw { kind: "network", message: "Failed to fetch" };
    const id = bodies.size + 1;
    bodies.set(id, answered.body);
    return { id, status: answered.status, headers: [] };
  }
  if (command === "plugin:remote|read") {
    const id = (rest[0] as { id: number }).id;
    const body = bodies.get(id);
    if (body === null) return new Promise(() => {});
    bodies.set(id, "");
    return new TextEncoder().encode(body ?? "").buffer;
  }
  return null;
}

/** The Access service tokens the app keeps, by server origin, as client IDs. */
const accessTokens = new Map<string, string>();

/** The app's side of the connection commands, with `server` in use. */
function appCommands(server: string | null) {
  native.invoke.mockImplementation(async (command: string, args?: unknown, ...rest: unknown[]) => {
    if (command.startsWith("plugin:remote|")) return remote(command, [args, ...rest]);
    if (command === "set_connection") return (args as { server: string | null }).server;
    if (command === "get_connection") return { ...saved, server };
    if (command === "save_server") {
      const { url, name } = args as { url: string; name: string };
      return { server, servers: [...saved.servers, { name, url }] };
    }
    if (command === "get_access_token") {
      const clientId = accessTokens.get((args as { server: string }).server);
      return clientId === undefined ? null : { clientId };
    }
    if (command === "set_access_token") {
      const { server: url, token } = args as { server: string; token: { clientId: string } | null };
      if (token === null) accessTokens.delete(url);
      else accessTokens.set(url, token.clientId);
      return null;
    }
    if (command === "remove_server") {
      const url = (args as { url: string }).url;
      return { server, servers: saved.servers.filter((item) => item.url !== url) };
    }
    throw new Error(`unexpected command ${command}`);
  });
}

beforeEach(() => {
  accessTokens.clear();
  bodies.clear();
  answer = () => ({ status: 200, body: "" });
  native.invoke.mockReset();
  native.relaunch.mockClear();
});

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
  sessionStorage.clear();
  delete window.__TOODO_SERVER__;
});

describe("switching servers", () => {
  it("reloads the window from one server to another, and the new window follows only that server", async () => {
    appCommands(A);
    const streams: { url: string; token: string | null }[] = [];
    answer = (request) => {
      const token = request.headers.find(([name]) => name === "authorization")?.[1] ?? null;
      streams.push({ url: request.url, token });
      return { status: 200, body: null };
    };
    localStorage.setItem(`toodo-server-token@${B}`, "device-b");
    const a = await startWindow(A);
    a.session.setToken("device-a");
    await a.event.listen("tasks-changed", () => {});
    await vi.waitFor(() => expect(streams).toHaveLength(1));
    expect(streams[0]).toMatchObject({ url: `${A}/api/events`, token: "Bearer device-a" });

    const reload = vi.fn();
    await a.servers.switchServer(B, reload);

    expect(native.invoke).toHaveBeenCalledWith("set_connection", { server: B });
    expect(reload).toHaveBeenCalledOnce();
    expect(native.relaunch).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(a.servers.SWITCHED_SERVER_KEY)).toBe(B);

    // The reloaded window starts with the server the init script reads back.
    const b = await startWindow(sessionStorage.getItem(a.servers.SWITCHED_SERVER_KEY));
    await b.event.listen("tasks-changed", () => {});
    await vi.waitFor(() => expect(streams).toHaveLength(2));
    expect(streams[1]).toMatchObject({ url: `${B}/api/events`, token: "Bearer device-b" });
    expect(b.session.getToken()).toBe("device-b");
  });

  it("restarts the app between a server and this computer's data", async () => {
    appCommands(A);
    const a = await startWindow(A);
    sessionStorage.setItem(a.servers.SWITCHED_SERVER_KEY, A);
    const reload = vi.fn();

    await a.servers.switchServer(null, reload);
    expect(native.relaunch).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(a.servers.SWITCHED_SERVER_KEY)).toBeNull();

    const local = await startWindow(null);
    await local.servers.switchServer(B, reload);
    expect(native.relaunch).toHaveBeenCalledTimes(2);
    expect(reload).not.toHaveBeenCalled();
  });

  it("stays on the server when the app refuses the switch", async () => {
    native.invoke.mockRejectedValue({ kind: "invalidInput", message: "\"ftp://x\" is not an http or https URL" });
    const a = await startWindow(A);
    const reload = vi.fn();

    await expect(a.servers.switchServer("ftp://x", reload)).rejects.toThrow("not an http or https URL");
    expect(reload).not.toHaveBeenCalled();
    expect(native.relaunch).not.toHaveBeenCalled();
    expect(sessionStorage.getItem(a.servers.SWITCHED_SERVER_KEY)).toBeNull();
  });
});

describe("one server offline", () => {
  it("names the unreachable server and offers the others, which do not wait for it", async () => {
    appCommands(A);
    answer = (request) => (request.url.startsWith(A) ? "network" : { status: 401, body: "" });
    const a = await startWindow(A);
    const { ConnectionGate } = await import("@/components/offline-screen");
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}>
        <ConnectionGate>
          <p>App</p>
        </ConnectionGate>
      </QueryClientProvider>,
    );

    await act(async () => {
      await a.connection.serverFetch("/api/rpc/list_tasks").catch(() => {});
    });
    await vi.waitFor(() => expect(a.connection.reachability()).toBe("unreachable"));
    expect(await screen.findByText("Can't reach Home")).toBeInTheDocument();
    const other = await screen.findByRole("button", { name: "Switch to Work" });
    expect(other).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Switch to Home" })).toBeNull();

    // Without a window to reload in jsdom, the switch is seen in what it leaves for the next window.
    fireEvent.click(other);
    await vi.waitFor(() => expect(sessionStorage.getItem("toodo-switched-server")).toBe(B));

    const b = await startWindow(B);
    await b.connection.serverFetch("/api");
    expect(b.connection.reachability()).toBe("reachable");
  });
});

describe("saved servers", () => {
  it("forget what the app kept for a removed server, and only for it", async () => {
    appCommands(A);
    localStorage.setItem(`toodo-server-token@${A}`, "device-a");
    localStorage.setItem(`toodo-chat-read@${A}`, "3");
    localStorage.setItem(`toodo-server-token@${B}`, "device-b");
    localStorage.setItem(`toodo-chat-read@${B}`, "9");
    localStorage.setItem("toodo-theme", "dark");
    const { ServerConnection } = await startWindow(A).then(() => import("@/components/server-connection"));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ServerConnection />
      </QueryClientProvider>,
    );

    expect(screen.queryByRole("button", { name: "Remove Home" })).toBeNull();
    fireEvent.click(await screen.findByRole("button", { name: "Remove Work" }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));

    await vi.waitFor(() => expect(screen.queryByText("Work")).toBeNull());
    expect(native.invoke).toHaveBeenCalledWith("remove_server", { url: B });
    expect(localStorage.getItem(`toodo-server-token@${B}`)).toBeNull();
    expect(localStorage.getItem(`toodo-chat-read@${B}`)).toBeNull();
    expect(localStorage.getItem(`toodo-server-token@${A}`)).toBe("device-a");
    expect(localStorage.getItem(`toodo-chat-read@${A}`)).toBe("3");
    expect(localStorage.getItem("toodo-theme")).toBe("dark");
  });

  it("keep an Access service token for each server, set and removed for that server only", async () => {
    appCommands(A);
    accessTokens.set(B, "work.access");
    const { ServerConnection } = await startWindow(A).then(() => import("@/components/server-connection"));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ServerConnection />
      </QueryClientProvider>,
    );

    expect(await screen.findByText("work.access")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove service token of Home" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Add service token of Home" }));
    const form = screen.getByRole("form", { name: "Service token of Home" });
    fireEvent.change(within(form).getByLabelText("Client ID"), { target: { value: "home.access" } });
    fireEvent.change(within(form).getByLabelText("Client secret"), { target: { value: "secret" } });
    fireEvent.click(within(form).getByRole("button", { name: "Save service token" }));

    expect(await screen.findByText("home.access")).toBeInTheDocument();
    expect(native.invoke).toHaveBeenCalledWith("set_access_token", {
      server: A,
      token: { clientId: "home.access", clientSecret: "secret" },
    });
    expect(accessTokens.get(B)).toBe("work.access");

    fireEvent.click(screen.getByRole("button", { name: "Remove service token of Work" }));
    await vi.waitFor(() => expect(screen.queryByText("work.access")).toBeNull());
    expect(native.invoke).toHaveBeenCalledWith("set_access_token", { server: B, token: null });
    expect(accessTokens.get(A)).toBe("home.access");
  });

  it("keep the service token entered with a new server for that server", async () => {
    appCommands(A);
    const C = "https://toodo.example.com";
    const { ServerConnection } = await startWindow(A).then(() => import("@/components/server-connection"));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ServerConnection />
      </QueryClientProvider>,
    );

    const form = await screen.findByRole("form", { name: "Add a Toodo server" });
    fireEvent.change(within(form).getByLabelText("Toodo server address"), { target: { value: C } });
    fireEvent.change(within(form).getByLabelText("Client ID"), { target: { value: "office.access" } });
    fireEvent.change(within(form).getByLabelText("Client secret"), { target: { value: "secret" } });
    fireEvent.click(within(form).getByRole("button", { name: "Add server" }));

    expect(await screen.findByText("office.access")).toBeInTheDocument();
    const calls = native.invoke.mock.calls.map(([command]) => command);
    expect(calls.indexOf("set_access_token")).toBeLessThan(calls.indexOf("save_server"));
    expect(accessTokens).toEqual(new Map([[C, "office.access"]]));
  });

  it("signs in to another server again without touching the server in use", async () => {
    appCommands(A);
    localStorage.setItem(`toodo-server-token@${A}`, "device-a");
    localStorage.setItem(`toodo-server-token@${B}`, "device-b");
    const { ServerConnection } = await startWindow(A).then(() => import("@/components/server-connection"));
    render(
      <QueryClientProvider client={new QueryClient()}>
        <ServerConnection />
      </QueryClientProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: "Sign in to Work again" }));

    expect(localStorage.getItem(`toodo-server-token@${B}`)).toBeNull();
    expect(localStorage.getItem(`toodo-server-token@${A}`)).toBe("device-a");
    expect(screen.queryByRole("button", { name: "Sign in to Work again" })).toBeNull();
    expect(screen.getByRole("button", { name: "Sign in to Home again" })).toBeInTheDocument();
  });
});
