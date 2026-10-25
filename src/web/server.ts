/**
 * The `toodo-server` the app talks to: where its API is, how to send it requests, and the
 * key its access token is stored under, so that each server keeps its own. The web build
 * talks to the server that serves it, with relative URLs; the desktop app connected to a server sends its requests to that
 * server's origin from Rust (see `src/client/transport.ts`), with the server's Cloudflare
 * Access service token if it has one.
 */
import { connectedServer } from "@/lib/platform";

export interface Server {
  /** The server's origin, or "" for the server that serves this page. */
  id: string;
  /** The URL of `path` (starting with "/") on the server. */
  url(path: string): string;
  /** The local storage key of `name` for this server. */
  key(name: string): string;
  /** `fetch` for `path` (starting with "/") on the server. */
  fetch(path: string, init?: RequestInit): Promise<Response>;
}

export function server(origin: string | null): Server {
  if (origin === null) {
    return { id: "", url: (path) => path, key: (name) => name, fetch: (path, init) => fetch(path, init) };
  }
  const url = (path: string) => `${origin}${path}`;
  return {
    id: origin,
    url,
    key: (name) => `${name}@${origin}`,
    // The web build has no app to send requests from, and leaves the transport out.
    fetch: async (path, init) =>
      import.meta.env.MODE === "web"
        ? fetch(url(path), init)
        : (await import("@/client/transport")).remoteFetch(url(path), init),
  };
}

/** The names of what the app keeps for each server (see `src/web/session.ts`). */
const perServerNames = ["toodo-server-token"] as const;

/** Forgets what the app keeps for the server at `origin`. */
export function forgetServer(origin: string): void {
  for (const name of perServerNames) {
    localStorage.removeItem(server(origin).key(name));
  }
}

/** The server this app talks to. */
export const activeServer: Server = server(connectedServer);
