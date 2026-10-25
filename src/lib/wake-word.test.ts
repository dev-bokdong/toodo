import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { WakeMessage } from "@/lib/wake-word";

const bridge = vi.hoisted(() => ({
  listeners: new Map<string, (payload: unknown) => void>(),
  microphone: "granted",
  pending: null as WakeMessage | null,
  reachable: () => {},
}));

vi.mock("@/lib/platform", () => ({ mobileApp: true }));
vi.mock("@/web/server", () => ({
  activeServer: { id: "https://one.example", key: (name: string) => `${name}@https://one.example` },
}));
vi.mock("@/web/connection", () => ({
  onReachable: (listener: () => void) => { bridge.reachable = listener; return () => {}; },
  reachability: () => "reachable",
  subscribeReachability: () => () => {},
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command.endsWith("Permissions")) return { microphone: bridge.microphone };
    if (command.endsWith("|pendingWake")) return bridge.pending;
    if (command.endsWith("|wakeState")) return { state: "off", error: null };
    return undefined;
  }),
  addPluginListener: vi.fn(async (_plugin: string, event: string, callback: (payload: unknown) => void) => {
    bridge.listeners.set(event, callback);
    return { unregister: vi.fn(() => bridge.listeners.delete(event)) };
  }),
}));

const message = { id: "fixed-id", text: "Start the next task", scope: "https://one.example" };
let disconnect: (() => void) | undefined;

function signal() {
  let resolve = () => {};
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("Android wake word delivery", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.mocked(invoke).mockClear();
    bridge.listeners.clear();
    bridge.microphone = "granted";
    bridge.pending = null;
    localStorage.clear();
  });

  afterEach(() => {
    disconnect?.();
    disconnect = undefined;
    vi.unstubAllGlobals();
  });

  it("defaults off and never requests permission on connection", async () => {
    const wake = await import("@/lib/wake-word");
    const configured = signal();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "off", error: null };
      if (command.endsWith("|pendingWake")) return null;
      if (command.endsWith("|configureWake")) configured.resolve();
    });

    disconnect = wake.connectWakeWord(vi.fn());
    await configured.promise;

    expect(wake.wakeEnabled()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("plugin:speech|configureWake", {
      enabled: false, suspended: false, scope: message.scope,
    });
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command.endsWith("Permissions"))).toBe(false);
  });

  it("leaves listening disabled when the microphone is denied", async () => {
    bridge.microphone = "denied";
    vi.mocked(invoke).mockImplementation(async (command) =>
      command.endsWith("Permissions") ? { microphone: bridge.microphone } : undefined,
    );
    const wake = await import("@/lib/wake-word");

    await wake.setWakeEnabled(true);

    expect(wake.wakeEnabled()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("plugin:speech|requestPermissions", { permissions: ["microphone"] });
    expect(invoke).toHaveBeenCalledWith("plugin:speech|configureWake", expect.objectContaining({ enabled: false }));
  });

  it("retries the persisted transcript and id after a lost server response", async () => {
    bridge.pending = message;
    const attempted = signal();
    const accepted = signal();
    const send = vi.fn()
      .mockImplementationOnce(async () => { attempted.resolve(); throw new Error("Connection lost"); })
      .mockImplementationOnce(async () => { accepted.resolve(); });
    const wake = await import("@/lib/wake-word");
    const view = renderHook(() => wake.useWakeState());
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "pending", error: null };
      if (command.endsWith("|pendingWake")) return bridge.pending;
    });

    await act(async () => {
      disconnect = wake.connectWakeWord(send);
      await attempted.promise;
    });
    expect(view.result.current.pending).toEqual(message);
    expect(view.result.current.sendError).toBe("Connection lost");
    expect(invoke).not.toHaveBeenCalledWith("plugin:speech|acknowledgeWake", expect.anything());

    await act(async () => { bridge.reachable(); await accepted.promise; });
    expect(send.mock.calls).toEqual([[message], [message]]);
    expect(invoke).toHaveBeenCalledWith("plugin:speech|acknowledgeWake", { id: message.id });
    expect(view.result.current.pending).toBeNull();
  });

  it("does not send a stored message to a different server", async () => {
    bridge.pending = { ...message, scope: "https://other.example" };
    const restored = signal();
    const wake = await import("@/lib/wake-word");
    const send = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "pending", error: null };
      if (command.endsWith("|pendingWake")) return bridge.pending;
      if (command.endsWith("|configureWake")) restored.resolve();
    });

    disconnect = wake.connectWakeWord(send);
    await restored.promise;

    expect(send).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalledWith("plugin:speech|acknowledgeWake", expect.anything());
  });

  it("keeps the native outbox and explains a failed durable acknowledgement", async () => {
    bridge.pending = message;
    const acknowledged = signal();
    const send = vi.fn().mockResolvedValue(undefined);
    const wake = await import("@/lib/wake-word");
    const view = renderHook(() => wake.useWakeState());
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "pending", error: null };
      if (command.endsWith("|pendingWake")) return bridge.pending;
      if (command.endsWith("|acknowledgeWake")) {
        acknowledged.resolve();
        throw { message: "Could not acknowledge wake message" };
      }
    });

    await act(async () => {
      disconnect = wake.connectWakeWord(send);
      await acknowledged.promise;
    });

    expect(view.result.current.pending).toEqual(message);
    expect(view.result.current.sendError).toBe("Could not acknowledge wake message");
  });

  it("does not post twice when the same pending message arrives during a send", async () => {
    const configured = signal();
    const response = signal();
    const acknowledged = signal();
    const send = vi.fn(() => response.promise);
    const wake = await import("@/lib/wake-word");
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "off", error: null };
      if (command.endsWith("|pendingWake")) return null;
      if (command.endsWith("|configureWake")) configured.resolve();
      if (command.endsWith("|acknowledgeWake")) acknowledged.resolve();
    });
    disconnect = wake.connectWakeWord(send);
    await configured.promise;

    bridge.listeners.get("wakeMessage")?.(message);
    bridge.listeners.get("wakeMessage")?.(message);

    expect(send).toHaveBeenCalledTimes(1);
    response.resolve();
    await acknowledged.promise;
  });

  it("keeps a recovery signal that arrives before the failed send finishes", async () => {
    const configured = signal();
    const restored = signal();
    const response = signal();
    const acknowledged = signal();
    bridge.pending = message;
    const send = vi.fn()
      .mockImplementationOnce(async () => { await response.promise; throw new Error("Connection lost"); })
      .mockResolvedValueOnce(undefined);
    const wake = await import("@/lib/wake-word");
    let reads = 0;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("|wakeState")) return { state: "pending", error: null };
      if (command.endsWith("|pendingWake")) {
        if (++reads === 2) restored.resolve();
        return bridge.pending;
      }
      if (command.endsWith("|configureWake")) configured.resolve();
      if (command.endsWith("|acknowledgeWake")) acknowledged.resolve();
    });
    disconnect = wake.connectWakeWord(send);
    await configured.promise;

    bridge.reachable();
    await restored.promise;
    response.resolve();
    await acknowledged.promise;

    expect(send.mock.calls).toEqual([[message], [message]]);
    expect(invoke).toHaveBeenCalledWith("plugin:speech|acknowledgeWake", { id: message.id });
  });

  it("keeps wake paused until both dictation and playback release it", async () => {
    const wake = await import("@/lib/wake-word");

    await wake.suspendWake("speech", true);
    await wake.suspendWake("dictation", true);
    await wake.suspendWake("speech", false);

    expect(invoke).toHaveBeenLastCalledWith("plugin:speech|configureWake", expect.objectContaining({ suspended: true }));
    await wake.suspendWake("dictation", false);
    expect(invoke).toHaveBeenLastCalledWith("plugin:speech|configureWake", expect.objectContaining({ suspended: false }));
  });

  it("waits for native capture to stop before speaking a reply", async () => {
    const paused = signal();
    const spoken = signal();
    const speak = vi.fn(() => spoken.resolve());
    vi.stubGlobal("speechSynthesis", { getVoices: () => [], speak, cancel: vi.fn() });
    vi.stubGlobal("SpeechSynthesisUtterance", class {
      constructor(readonly text: string) {}
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command.endsWith("|configureWake") && args && "suspended" in args && args.suspended) {
        await paused.promise;
      }
    });
    const voice = await import("@/lib/voice");

    voice.speak("Here is the next task.");
    expect(speak).not.toHaveBeenCalled();
    paused.resolve();
    await spoken.promise;

    expect(speak).toHaveBeenCalledTimes(1);
    voice.stopSpeaking();
  });

  it("offers an off-by-default setting and keeps it off after permission denial", async () => {
    bridge.microphone = "denied";
    const configured = signal();
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command.endsWith("Permissions")) return { microphone: bridge.microphone };
      if (command.endsWith("|wakeState")) return { state: "off", error: null };
      if (command.endsWith("|pendingWake")) return null;
      if (command.endsWith("|configureWake")) {
        bridge.listeners.get("wakeState")?.({ state: "off", error: null });
        configured.resolve();
      }
    });
    const wake = await import("@/lib/wake-word");
    disconnect = wake.connectWakeWord(vi.fn());
    await configured.promise;
    const { WakeWordSettings } = await import("@/components/wake-word");
    render(createElement(WakeWordSettings));
    const checkbox = screen.getByRole("checkbox", { name: "Listen for Dori (do-ri-ya)" });
    expect(checkbox).not.toBeChecked();

    await act(async () => { fireEvent.click(checkbox); });

    expect(checkbox).not.toBeChecked();
    expect(screen.getByRole("alert")).toHaveTextContent("Microphone access is blocked");
    expect(screen.getByRole("button", { name: "Open app settings" })).toBeEnabled();
  });
});
