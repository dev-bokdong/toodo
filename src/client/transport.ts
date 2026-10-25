/**
 * Requests of the desktop app connected to a `toodo-server`: the app sends them from Rust
 * (see `src-tauri/src/remote/`), which adds the server's Cloudflare Access service token,
 * so the secret never enters the window. `remoteFetch` stands in for `fetch`, and
 * `openRemoteSocket` for a `WebSocket`, which a page cannot give headers.
 */
import { invoke } from "@tauri-apps/api/core";

type Failure = { kind: "access" | "network" | "invalid"; message: string };
type Head = { id: number; status: number; headers: [string, string][] };

/** Cloudflare Access in front of the server answered instead of it. */
export class AccessBlocked extends Error {
  override name = "AccessBlocked";
}

function isFailure(value: unknown): value is Failure {
  return typeof value === "object" && value !== null && "kind" in value && "message" in value;
}

/** The error a failed plugin call throws: a network failure is a `TypeError`, like `fetch`'s. */
function failure(error: unknown): Error {
  if (!isFailure(error)) {
    return error instanceof Error ? error : new Error(String(error));
  }
  if (error.kind === "access") return new AccessBlocked(error.message);
  if (error.kind === "network") return new TypeError(error.message);
  return new Error(error.message);
}

const NO_BODY = new Set([101, 204, 205, 304]);

export async function remoteFetch(url: string, init: RequestInit = {}): Promise<Response> {
  // The signal stays here: the app cancels the request itself.
  const request = new Request(url, { ...init, signal: null });
  const headers: [string, string][] = [];
  request.headers.forEach((value, name) => headers.push([name, value]));
  const body = new Uint8Array(await request.arrayBuffer());
  const signal = init.signal;
  signal?.throwIfAborted();
  let head: Head;
  try {
    head = await invoke<Head>("plugin:remote|fetch", body, {
      headers: { "toodo-request": JSON.stringify({ url: request.url, method: request.method, headers }) },
    });
  } catch (error) {
    throw failure(error);
  }
  const cancel = () => void invoke("plugin:remote|cancel", { id: head.id });
  if (signal?.aborted) {
    cancel();
    signal.throwIfAborted();
  }
  signal?.addEventListener("abort", cancel, { once: true });
  if (NO_BODY.has(head.status)) {
    cancel();
    return new Response(null, { status: head.status, headers: head.headers });
  }
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const chunk = new Uint8Array(await invoke<ArrayBuffer>("plugin:remote|read", { id: head.id }));
        if (chunk.byteLength === 0) {
          signal?.removeEventListener("abort", cancel);
          controller.close();
        } else {
          controller.enqueue(chunk);
        }
      } catch (error) {
        controller.error(failure(error));
      }
    },
    cancel,
  });
  return new Response(stream, { status: head.status, headers: head.headers });
}

/** A WebSocket opened by the app: the part of `WebSocket` the pane view uses. */
export class RemoteSocket {
  readyState: number = WebSocket.OPEN;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent<string>) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;

  constructor(private readonly id: number) {
    // Like a WebSocket's, the events come after whoever opened it set its handlers.
    setTimeout(() => {
      this.onopen?.(new Event("open"));
      void this.receive();
    }, 0);
  }

  private async receive() {
    for (;;) {
      let text: string | null;
      try {
        text = await invoke<string | null>("plugin:remote|socket_receive", { id: this.id });
      } catch {
        text = null;
      }
      if (text === null) break;
      this.onmessage?.(new MessageEvent("message", { data: text }));
    }
    this.closed();
  }

  private closed() {
    if (this.readyState === WebSocket.CLOSED) return;
    this.readyState = WebSocket.CLOSED;
    this.onclose?.(new CloseEvent("close"));
  }

  send(text: string): void {
    if (this.readyState !== WebSocket.OPEN) return;
    invoke("plugin:remote|socket_send", { id: this.id, text }).catch(() => this.close());
  }

  close(): void {
    if (this.readyState === WebSocket.CLOSED) return;
    void invoke("plugin:remote|socket_close", { id: this.id });
    this.closed();
  }
}

/** Opens a WebSocket to `url`, an `http` or `https` URL on the server. */
export async function openRemoteSocket(url: string): Promise<RemoteSocket> {
  try {
    return new RemoteSocket(await invoke<number>("plugin:remote|socket_open", { url }));
  } catch (error) {
    throw failure(error);
  }
}
