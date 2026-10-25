/**
 * Spoken messages on their way to the lead chat: what was dictated in the chat and what was
 * said after the wake word. They are sent here, outside any component, so a message goes out
 * with the chat closed or the window hidden in the menu bar, and a second one said while the
 * first is still sending is queued rather than dropped. Each keeps the client ID it got when
 * it was heard, so a retry after a failed send cannot post it twice, and its text stays
 * visible until the send succeeds.
 */
import { useEffect, useSyncExternalStore } from "react";
import { commands, type ChatMessage } from "@/lib/bindings";
import { newClientId } from "@/lib/chat";

export type OutgoingVoice = {
  clientId: string;
  text: string;
  state: "sending" | "failed";
  error: string | null;
};

/** Waits before the automatic retries of a failed send; after the last, only Retry sends. */
export const RETRY_MS = [2000, 5000, 15000, 30000];

let queue: OutgoingVoice[] = [];
let busy = false;
let failures = 0;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
const listeners = new Set<() => void>();
const sentListeners = new Set<(message: ChatMessage) => void>();

function update(next: OutgoingVoice[]) {
  queue = next;
  listeners.forEach((listener) => listener());
}

function patch(clientId: string, change: Partial<OutgoingVoice>) {
  update(queue.map((item) => (item.clientId === clientId ? { ...item, ...change } : item)));
}

/** Sends the first message, then the next, so they reach the lead in the order spoken. */
async function pump() {
  const head = queue[0];
  if (busy || head === undefined || retryTimer !== undefined) return;
  busy = true;
  patch(head.clientId, { state: "sending", error: null });
  let error: string | null = null;
  try {
    const result = await commands.sendChatMessage(head.text, head.clientId);
    if (result.status === "ok") {
      failures = 0;
      update(queue.filter((item) => item.clientId !== head.clientId));
      sentListeners.forEach((listener) => listener(result.data));
    } else {
      error = result.error.message;
    }
  } catch (cause) {
    error = cause instanceof Error ? cause.message : String(cause);
  }
  busy = false;
  if (error !== null) {
    patch(head.clientId, { state: "failed", error });
    const wait = RETRY_MS[failures];
    failures += 1;
    if (wait === undefined) return;
    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      void pump();
    }, wait);
    return;
  }
  void pump();
}

/** Queues a spoken message for the lead; blank text is not sent. */
export function sendVoiceMessage(text: string, clientId: string = newClientId()) {
  const trimmed = text.trim();
  if (!trimmed || queue.some((item) => item.clientId === clientId)) return;
  update([...queue, { clientId, text: trimmed, state: "sending", error: null }]);
  void pump();
}

/** Sends a failed message again at once, under the same client ID. */
export function retryVoiceMessage(clientId: string) {
  if (!queue.some((item) => item.clientId === clientId)) return;
  clearTimeout(retryTimer);
  retryTimer = undefined;
  failures = 0;
  if (queue[0]?.clientId !== clientId) {
    // A message retried out of turn goes first; the order spoken matters less than it.
    const item = queue.find((entry) => entry.clientId === clientId)!;
    update([item, ...queue.filter((entry) => entry.clientId !== clientId)]);
  }
  void pump();
}

/** Gives up on a message that was not sent. */
export function discardVoiceMessage(clientId: string) {
  if (busy && queue[0]?.clientId === clientId) return;
  update(queue.filter((item) => item.clientId !== clientId));
  if (queue.length === 0) {
    clearTimeout(retryTimer);
    retryTimer = undefined;
    failures = 0;
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useVoiceOutbox(): OutgoingVoice[] {
  return useSyncExternalStore(subscribe, () => queue);
}

/** Calls `listener` with each message the server took. */
export function useVoiceSent(listener: (message: ChatMessage) => void) {
  useEffect(() => {
    sentListeners.add(listener);
    return () => void sentListeners.delete(listener);
  }, [listener]);
}
