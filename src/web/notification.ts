/** Stands in for `@tauri-apps/plugin-notification` in the web build: browser notifications. */

export async function isPermissionGranted(): Promise<boolean> {
  return "Notification" in window && Notification.permission === "granted";
}

export async function requestPermission(): Promise<NotificationPermission> {
  return "Notification" in window ? Notification.requestPermission() : "denied";
}

export function sendNotification(options: { title: string; body?: string }): void {
  new Notification(options.title, { body: options.body });
}
