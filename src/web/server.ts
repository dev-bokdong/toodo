/**
 * The `toodo-server` the app talks to: where its API is, and under which keys what the app
 * keeps for it (its access token, the read mark of the chat) is stored, so that each server
 * keeps its own. The web build talks to the server that serves it, with relative URLs; the
 * desktop app connected to a server talks to that server's origin.
 */
import { connectedServer } from "@/lib/platform";

export interface Server {
  /** The server's origin, or "" for the server that serves this page. */
  id: string;
  /** The URL of `path` (starting with "/") on the server. */
  url(path: string): string;
  /** The local storage key of `name` for this server. */
  key(name: string): string;
}

export function server(origin: string | null): Server {
  if (origin === null) {
    return { id: "", url: (path) => path, key: (name) => name };
  }
  return { id: origin, url: (path) => `${origin}${path}`, key: (name) => `${name}@${origin}` };
}

/** The server this app talks to. */
export const activeServer: Server = server(connectedServer);
