/**
 * Links into the Android app: `toodo://chat` opens the lead chat, and `toodo://chat/voice`
 * opens it listening for a spoken message (the launcher shortcut uses it). A link names no
 * server and carries no token: the app uses the server it is connected to. A request waits
 * until the chat is shown, which is once the server answers and the app is signed in.
 */
export type ChatRequest = { voice: boolean };

let pending: ChatRequest | null = null;
const listeners = new Set<() => void>();

export function parseAppLink(url: string): ChatRequest | null {
  const match = /^toodo:\/\/chat(\/voice)?\/?(?:[?#].*)?$/i.exec(url.trim());
  return match ? { voice: match[1] !== undefined } : null;
}

export function requestChat(request: ChatRequest) {
  pending = request;
  listeners.forEach((listener) => listener());
}

export function takeChatRequest(): ChatRequest | null {
  const request = pending;
  pending = null;
  return request;
}

export function onChatRequest(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Follows the links that start the app (cold) and that reach it while it runs (warm). */
export async function followAppLinks() {
  const { getCurrent, onOpenUrl } = await import("@tauri-apps/plugin-deep-link");
  const open = (urls: string[] | null) => {
    for (const url of urls ?? []) {
      const request = parseAppLink(url);
      if (request) requestChat(request);
    }
  };
  await onOpenUrl(open);
  open(await getCurrent());
}
