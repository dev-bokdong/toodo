/**
 * `@tauri-apps/api/core` for the app's own code in the desktop build (see `vite.config.ts`).
 * Connected to a `toodo-server`, the app runs its commands there; the commands about the
 * app itself and the calls of Tauri plugins stay in the app.
 */
import { invoke as invokeInApp, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { connectedServer } from "@/lib/platform";
import { invoke as invokeOnServer } from "@/web/core";

export * from "@tauri-apps/api/core";

/** Commands of the desktop app that the server does not have. */
const APP_COMMANDS = new Set([
  "get_connection",
  "set_connection",
  "save_server",
  "remove_server",
  "get_access_token",
  "set_access_token",
  "start_dictation",
  "stop_dictation",
  "cancel_dictation",
  "open_privacy_settings",
]);

export function runsInApp(command: string): boolean {
  return connectedServer === null || command.startsWith("plugin:") || APP_COMMANDS.has(command);
}

export function invoke<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  if (runsInApp(command)) {
    return invokeInApp<T>(command, args, options);
  }
  return invokeOnServer<T>(command, args as Record<string, unknown> | undefined);
}
