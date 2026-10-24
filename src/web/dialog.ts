/**
 * Stands in for `@tauri-apps/plugin-dialog` in the web build, which has no file dialogs:
 * it browses the server's folders in the app, downloads files in the browser, and uploads
 * picked files instead. Nothing in the web build calls these.
 */

function unavailable(): Promise<never> {
  return Promise.reject(new Error("The web app has no file dialogs."));
}

export const open = unavailable;

export const save = unavailable;
