/** Stands in for `@tauri-apps/plugin-process` in the web build. */

export async function relaunch(): Promise<void> {
  window.location.reload();
}
