/**
 * The WebSocket of a pane view on `toodo-server`: it first gets a single-use ticket for the
 * pane with the access token, since a browser cannot send the token on a WebSocket.
 */
import { activeServer } from "@/web/server";
import { apiRequest } from "@/web/session";

export type PaneServerMessage =
  | { type: "frame"; seq: number; cols: number; rows: number; ansi: string }
  | { type: "control"; on: boolean }
  | { type: "error"; message: string }
  | { type: "closed"; reason: string };

export type PaneClientMessage =
  | { type: "control"; on: boolean }
  | { type: "text"; text: string }
  | { type: "keys"; keys: string[] };

async function ticket(taskId: number, paneId: string): Promise<string> {
  const response = await apiRequest("/api/stream-tickets", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ taskId, paneId }),
  });
  return ((await response.json()) as { ticket: string }).ticket;
}

/** The part of a `WebSocket` the pane view uses. */
export interface PaneSocket {
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onmessage: ((event: MessageEvent<string>) => void) | null;
  onclose: ((event: CloseEvent) => void) | null;
  send(text: string): void;
  close(): void;
}

/** Opens the stream of pane `paneId` of the herdr workspace of task `taskId`. */
export async function openPaneStream(taskId: number, paneId: string): Promise<PaneSocket> {
  const url = new URL(activeServer.url("/api/panes/stream"), window.location.href);
  url.searchParams.set("ticket", await ticket(taskId, paneId));
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return new WebSocket(url);
}
