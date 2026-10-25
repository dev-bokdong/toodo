/**
 * The wake word of the macOS app (`src-tauri/src/dictation/wake/`): the app listens for
 * "Dori-ya" itself, also while its window is hidden, and reports what it hears. This keeps
 * its state for the settings and the listening indicator, and hands each sentence heard to
 * the voice outbox, which sends it to the lead chat.
 */
import { useSyncExternalStore } from "react";
import { commands, events, type WakeWordChanged } from "@/lib/bindings";
import { connectedServer, nativeDictation } from "@/lib/platform";
import { sendVoiceMessage } from "@/lib/voice-outbox";

/** Whether this app offers the wake word: the macOS app connected to a server with a lead. */
export const wakeWordOffered = nativeDictation && connectedServer !== null;

let state: WakeWordChanged = { enabled: false, status: { state: "off" } };
let started = false;
const listeners = new Set<() => void>();

function set(next: WakeWordChanged) {
  state = next;
  listeners.forEach((listener) => listener());
}

/**
 * Follows the app's wake word and sends what it hears; called once by the app at start.
 * Returns a function that stops following it.
 */
export function followWakeWord(): () => void {
  if (started) return () => {};
  started = true;
  const subscriptions = [
    events.wakeWordChanged.listen(({ payload }) => set(payload)),
    events.wakeWordHeard.listen(({ payload }) => sendVoiceMessage(payload.message)),
  ];
  void commands.getWakeWord().then((result) => {
    if (result.status === "ok") set(result.data);
  });
  return () => {
    started = false;
    subscriptions.forEach((subscription) => void subscription.then((unlisten) => unlisten()));
  };
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useWakeWord(): WakeWordChanged {
  return useSyncExternalStore(subscribe, () => state);
}

export async function setWakeWordEnabled(enabled: boolean) {
  const result = await commands.setWakeWord(enabled);
  if (result.status === "error") throw new Error(result.error.message);
  set(result.data);
}

export function cancelWakeWord() {
  void commands.cancelWakeWord();
}

/** Tells the app whether a reply is read aloud, so Dori's own voice cannot wake it. */
export function setWakeWordSpeaking(speaking: boolean) {
  void commands.setWakeWordSpeaking(speaking);
}
