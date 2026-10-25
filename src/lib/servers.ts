/**
 * The servers saved in the desktop app (see `src-tauri/src/connection.rs`) and switching
 * between them. From one server to another, the window only reloads: the page takes the
 * old server's cached data, event stream, pane streams, and retry timers with it, and the
 * new page starts with the other server's token. Between a server and this computer's own
 * data, the app restarts, since it then starts or stops its own data and background work.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { relaunch } from "@tauri-apps/plugin-process";
import { commands, type AccessTokenInput, type SavedServer } from "@/lib/bindings";
import { builtForWeb, connectedServer } from "@/lib/platform";
import { unwrap } from "@/lib/queries";
import { activeServer, forgetServer, server } from "@/web/server";
import { forgetToken } from "@/web/session";

/** Where the window keeps the server it switched to; the app's init script reads it on reload. */
export const SWITCHED_SERVER_KEY = "toodo-switched-server";

const connectionKey = ["connection"] as const;
const accessTokenKey = (url: string) => ["access-token", url] as const;

/** The server in use (null for this computer's data) and the servers saved in the app. */
export type Connection = { server: string | null; servers: SavedServer[] };

async function connection(result: ReturnType<typeof commands.getConnection>): Promise<Connection> {
  const { server, servers } = await unwrap(result);
  return { server, servers: servers ?? [] };
}

export function useConnection() {
  return useQuery({
    queryKey: connectionKey,
    queryFn: () => connection(commands.getConnection()),
    // The desktop app's own command: only the desktop app has saved servers.
    enabled: !builtForWeb,
  });
}

/** The name `origin` is saved under, or its host while the saved servers are not known. */
export function serverName(connection: Connection | undefined, origin: string): string {
  return connection?.servers.find((saved) => saved.url === origin)?.name ?? new URL(origin).host;
}

export function signedIn(origin: string): boolean {
  return localStorage.getItem(server(origin).key("toodo-server-token")) !== null;
}

/**
 * Forgets the app's token for the server at `origin`, so that the app signs in to it again
 * (at once for the server in use, else when it switches to it).
 */
export function signOut(origin: string): void {
  if (origin === activeServer.id) {
    forgetToken();
  } else {
    localStorage.removeItem(server(origin).key("toodo-server-token"));
  }
}

/** Makes the app use `target` (a server address, or null for this computer's data). */
export async function switchServer(target: string | null, reload = () => window.location.reload()): Promise<void> {
  const next = await unwrap(commands.setConnection(target));
  if (connectedServer !== null && next !== null) {
    sessionStorage.setItem(SWITCHED_SERVER_KEY, next);
    reload();
    return;
  }
  sessionStorage.removeItem(SWITCHED_SERVER_KEY);
  await relaunch();
}

export function useSaveServer() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ url, name }: { url: string; name: string }) => connection(commands.saveServer(url, name)),
    onSuccess: (connection) => client.setQueryData(connectionKey, connection),
  });
}

export function useRemoveServer() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (url: string) => connection(commands.removeServer(url)),
    onSuccess: (saved, url) => {
      forgetServer(url);
      // The app forgets the server's Access service token with it.
      client.removeQueries({ queryKey: accessTokenKey(url) });
      client.setQueryData(connectionKey, saved);
    },
  });
}

/** The client ID of the Cloudflare Access service token the app keeps for the server at `url`. */
export function useAccessToken(url: string) {
  return useQuery({
    queryKey: accessTokenKey(url),
    queryFn: () => unwrap(commands.getAccessToken(url)),
    enabled: !builtForWeb,
  });
}

/** Keeps `token` as the Access service token of the server at `url`, or forgets it for null. */
export function useSetAccessToken() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: ({ url, token }: { url: string; token: AccessTokenInput | null }) =>
      unwrap(commands.setAccessToken(url, token)),
    onSuccess: (_, { url }) => client.invalidateQueries({ queryKey: accessTokenKey(url) }),
  });
}
