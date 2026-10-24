/** Stands in for `@tauri-apps/plugin-notification` in the web build: browser notifications. */
import { pushActive } from "@/web/push";

export async function isPermissionGranted(): Promise<boolean> {
  return "Notification" in window && Notification.permission === "granted";
}

export async function requestPermission(): Promise<NotificationPermission> {
  return "Notification" in window ? Notification.requestPermission() : "denied";
}

/**
 * Shows a notification, unless the server pushes alerts to this device: its push brings the
 * same alert, which would then show twice.
 */
export function sendNotification(options: { title: string; body?: string }): void {
  if (pushActive()) {
    return;
  }
  new Notification(options.title, { body: options.body });
}
