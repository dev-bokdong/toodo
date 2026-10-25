import { useSyncExternalStore } from "react";
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";
import { requestChat } from "@/lib/app-links";
import { microphoneAllowed } from "@/lib/native-speech";
import { mobileApp } from "@/lib/platform";
import { onReachable } from "@/web/connection";
import { activeServer } from "@/web/server";

const ENABLED_KEY = "toodo-android-wake-word";

export type WakeMessage = { readonly id: string; readonly text: string; readonly scope: string };
type NativeState = {
  readonly state: "off" | "preparing" | "listening" | "capturing" | "paused" | "pending" | "error";
  readonly error: string | null;
};
type Snapshot = NativeState & {
  readonly pending: WakeMessage | null;
  readonly sending: boolean;
  readonly sendError: string | null;
};

let snapshot: Snapshot = { state: "off", error: null, pending: null, sending: false, sendError: null };
const listeners = new Set<() => void>();
const suspended = new Set<"dictation" | "speech">();
let sendMessage: ((message: WakeMessage) => Promise<unknown>) | null = null;
let configuration: Promise<void> = Promise.resolve();
let enabling = 0;
let permissionError: string | null = null;
let retryRequested = false;

function publish(change: Partial<Snapshot>) {
  snapshot = { ...snapshot, ...change, error: permissionError ?? (change.error === undefined ? snapshot.error : change.error) };
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function wakeEnabled(): boolean {
  return mobileApp && localStorage.getItem(ENABLED_KEY) === "on";
}

export function useWakeEnabled(): boolean {
  return useSyncExternalStore(subscribe, wakeEnabled);
}

export function useWakeState(): Snapshot {
  return useSyncExternalStore(subscribe, () => snapshot);
}

function errorText(cause: unknown): string {
  if (cause instanceof Error) return cause.message;
  // Android Invoke.reject sends a JSON object, not a JavaScript Error.
  if (typeof cause === "object" && cause !== null && "message" in cause && typeof cause.message === "string") {
    return cause.message;
  }
  return String(cause);
}

function configure(): Promise<void> {
  if (!mobileApp) return Promise.resolve();
  const args = {
    enabled: sendMessage !== null && wakeEnabled(),
    suspended: suspended.size > 0,
    scope: activeServer.id,
  };
  const next = configuration.then(() => invoke<void>("plugin:speech|configureWake", args));
  // Keep later stop/resume commands usable after a failed start, but report the failure.
  configuration = next.catch((cause: unknown) => publish({ state: "error", error: errorText(cause) }));
  return next;
}

/** Called by the setting only; resume never opens a permission prompt. */
export async function setWakeEnabled(enabled: boolean): Promise<void> {
  if (!mobileApp) return;
  const attempt = ++enabling;
  try {
    if (enabled && !(await microphoneAllowed())) {
      if (attempt === enabling) {
        localStorage.removeItem(ENABLED_KEY);
        permissionError = "Microphone access is blocked. Allow it in Android's app settings.";
        publish({ state: "error" });
        await configure();
      }
      return;
    }
    if (attempt !== enabling) return;
    permissionError = null;
    localStorage.setItem(ENABLED_KEY, enabled ? "on" : "off");
    publish({ error: null });
    await configure();
  } catch (cause) {
    publish({ state: "error", error: errorText(cause) });
  }
}

/** Await this before playback or manual dictation takes the microphone. */
export function suspendWake(reason: "dictation" | "speech", pause: boolean): Promise<void> {
  if (!mobileApp) return Promise.resolve();
  if (pause) suspended.add(reason);
  else suspended.delete(reason);
  return configure();
}

export function resumeWake(reason: "dictation" | "speech"): void {
  void suspendWake(reason, false).catch((cause: unknown) => {
    publish({ state: "error", error: errorText(cause) });
  });
}

export function cancelWake(): void {
  void invoke("plugin:speech|cancelWake").catch((cause: unknown) => {
    publish({ state: "error", error: errorText(cause) });
  });
}

/** The native outbox is cleared only after the server has accepted this exact client id. */
export async function retryWakeMessage(): Promise<void> {
  const message = snapshot.pending;
  const send = sendMessage;
  if (!message || !send || message.scope !== activeServer.id) return;
  if (snapshot.sending) {
    retryRequested = true;
    return;
  }
  retryRequested = false;
  publish({ sending: true, sendError: null });
  try {
    await send(message);
    await invoke("plugin:speech|acknowledgeWake", { id: message.id });
    publish({ pending: null });
  } catch (cause) {
    publish({ sendError: errorText(cause) });
  } finally {
    publish({ sending: false });
    if (retryRequested && snapshot.pending) void retryWakeMessage();
  }
}

/** One subscription for the always-mounted chat, including while its panel is closed. */
export function connectWakeWord(send: (message: WakeMessage) => Promise<unknown>): () => void {
  if (!mobileApp) return () => {};
  sendMessage = send;
  let active = true;
  const subscriptions: PluginListener[] = [];
  const received = (message: WakeMessage | null) => {
    if (!active || message === null) return;
    publish({ pending: message });
    requestChat({ voice: false, focusInput: false });
    void retryWakeMessage();
  };
  const restore = async () => {
    if (!active || document.visibilityState === "hidden") return;
    received(await invoke<WakeMessage | null>("plugin:speech|pendingWake"));
  };
  const refresh = () => {
    void restore().catch((cause: unknown) => publish({ sendError: errorText(cause) }));
  };
  const reachable = onReachable(refresh);
  document.addEventListener("visibilitychange", refresh);
  void (async () => {
    const stateSubscription = await addPluginListener<NativeState>("speech", "wakeState", (state) => {
      if (!active) return;
      publish(state);
      if (state.state === "capturing") requestChat({ voice: false, focusInput: false });
    });
    if (!active) {
      await stateSubscription.unregister();
      return;
    }
    subscriptions.push(stateSubscription);
    const subscription = await addPluginListener<WakeMessage>("speech", "wakeMessage", received);
    if (!active) {
      await subscription.unregister();
      return;
    }
    subscriptions.push(subscription);
    const state = await invoke<NativeState>("plugin:speech|wakeState");
    if (!active) return;
    publish(state);
    await restore();
    if (active) await configure();
  })().catch((cause: unknown) => {
    if (active) publish({ state: "error", error: errorText(cause) });
  });
  return () => {
    active = false;
    sendMessage = null;
    reachable();
    document.removeEventListener("visibilitychange", refresh);
    subscriptions.forEach((subscription) => void subscription.unregister());
    void configure().catch((cause: unknown) => publish({ state: "error", error: errorText(cause) }));
  };
}
