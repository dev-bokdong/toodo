/**
 * Stands in for `@tauri-apps/api/core` in the web build: the generated bindings invoke
 * commands through `invoke`, which posts them to `toodo-server`.
 */
import { serverFetch } from "@/web/connection";
import { authorization, forgetToken, getToken } from "@/web/session";

type CommandError = { kind: string; message: string };

function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as CommandError).kind === "string" &&
    typeof (value as CommandError).message === "string"
  );
}

/**
 * Runs a command on the server. Like Tauri's `invoke`, it resolves with the result and
 * rejects with the command's error object (not an `Error`), which the bindings turn into
 * `{ status: "error" }`; transport failures reject with an `Error`.
 */
export async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const token = getToken();
  if (token === null) {
    throw new Error("No access token for the Toodo server.");
  }
  const response = await serverFetch(`/api/rpc/${encodeURIComponent(command)}`, {
    method: "POST",
    headers: { ...authorization(token), "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  if (response.status === 401) {
    forgetToken();
    throw new Error("The Toodo server did not accept the access token.");
  }
  const text = await response.text();
  if (response.ok) {
    return (text ? JSON.parse(text) : null) as T;
  }
  let body: unknown = null;
  try {
    body = JSON.parse(text);
  } catch {
    // Not JSON: the server's own message, e.g. an unknown command.
  }
  if (isCommandError(body)) {
    throw body;
  }
  throw new Error(text || `${response.status} ${response.statusText}`);
}

/** Only plugins with native code send events this way, and the web build has none. */
export function addPluginListener(): Promise<never> {
  return Promise.reject(new Error("Native plugins only run in the Toodo apps."));
}
