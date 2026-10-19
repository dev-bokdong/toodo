/**
 * Stands in for `@tauri-apps/plugin-dialog` in the web build. Paths are paths on the
 * server, which the browser cannot browse, so the user types them.
 */

type DialogOptions = { title?: string; defaultPath?: string };

function ask(options: DialogOptions): string | null {
  const path = window.prompt(`${options.title ?? "Path"} (path on the server)`, options.defaultPath ?? "");
  return path?.trim() ? path.trim() : null;
}

export async function open(options: DialogOptions = {}): Promise<string | null> {
  return ask(options);
}

export async function save(options: DialogOptions = {}): Promise<string | null> {
  return ask(options);
}
