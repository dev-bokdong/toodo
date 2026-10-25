/**
 * `@tauri-apps/api/event` for the app's own code in the desktop build (see `vite.config.ts`).
 * Connected to a `toodo-server`, the app gets the server's events from its event stream;
 * the events of the app itself (Tauri's own and the tray's requests) still come from the app.
 */
import { listen as listenInApp, once as onceInApp, type EventCallback, type EventName, type Options } from "@tauri-apps/api/event";
import { connectedServer } from "@/lib/platform";
import { listen as listenOnServer, once as onceOnServer } from "@/web/event";

export * from "@tauri-apps/api/event";

/** Events the app emits itself, which the server does not send. */
const APP_EVENTS = new Set([
  "update-check-requested",
]);

export function comesFromApp(event: EventName): boolean {
  return connectedServer === null || event.startsWith("tauri://") || APP_EVENTS.has(event);
}

export function listen<T>(event: EventName, handler: EventCallback<T>, options?: Options) {
  return comesFromApp(event) ? listenInApp<T>(event, handler, options) : listenOnServer<T>(event, handler);
}

export function once<T>(event: EventName, handler: EventCallback<T>, options?: Options) {
  return comesFromApp(event) ? onceInApp<T>(event, handler, options) : onceOnServer<T>(event, handler);
}
