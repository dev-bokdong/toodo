import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { NativeRecognition, withNativeFallback, type Recognition } from "@/lib/native-speech";

const listeners = new Map<string, (payload: unknown) => void>();
let microphone = { checked: "granted", requested: "granted" };

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "plugin:speech|checkPermissions") return { microphone: microphone.checked };
    if (command === "plugin:speech|requestPermissions") return { microphone: microphone.requested };
    return null;
  }),
  addPluginListener: vi.fn(async (_plugin: string, event: string, handler: (payload: unknown) => void) => {
    listeners.set(event, handler);
    return { unregister: vi.fn() };
  }),
}));

const mockedInvoke = vi.mocked(invoke);

function scriptedWebRecognition(script: (web: Recognition) => void) {
  return class implements Recognition {
    lang = "";
    interimResults = false;
    continuous = false;
    maxAlternatives = 1;
    onresult: Recognition["onresult"] = null;
    onerror: Recognition["onerror"] = null;
    onend: Recognition["onend"] = null;
    start() {
      script(this);
    }
    stop() {}
    abort() {}
  };
}

function watch(recognition: Recognition) {
  const heard: string[] = [];
  const errors: string[] = [];
  let ended = 0;
  recognition.onresult = (event) => heard.push(event.results[0][0].transcript);
  recognition.onerror = (event) => errors.push(event.error);
  recognition.onend = () => (ended += 1);
  return { heard, errors, ended: () => ended };
}

function speechCalls() {
  return mockedInvoke.mock.calls.map(([command]) => command);
}

describe("native speech recognition", () => {
  beforeEach(() => {
    listeners.clear();
    mockedInvoke.mockClear();
    microphone = { checked: "granted", requested: "granted" };
  });

  it("hands over to the phone's recognition when the WebView's fails before hearing anything", async () => {
    const Recognition = withNativeFallback(
      scriptedWebRecognition((web) => {
        web.onerror?.({ error: "service-not-allowed" });
        web.onend?.();
      }),
    );
    const recognition = new Recognition();
    recognition.lang = "ko-KR";
    recognition.interimResults = true;
    const seen = watch(recognition);

    recognition.start();
    await vi.waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith("plugin:speech|start", { lang: "ko-KR", partialResults: true }),
    );
    listeners.get("result")?.({ text: "start the next task", final: true });
    listeners.get("end")?.({});

    expect(seen.heard).toEqual(["start the next task"]);
    expect(seen.errors).toEqual([]);
    expect(seen.ended()).toBe(1);
  });

  it("keeps the WebView's recognition once it has heard something", () => {
    const Recognition = withNativeFallback(
      scriptedWebRecognition((web) => {
        web.onresult?.({ results: [{ isFinal: false, 0: { transcript: "start" } }] });
        web.onerror?.({ error: "network" });
        web.onend?.();
      }),
    );
    const recognition = new Recognition();
    const seen = watch(recognition);

    recognition.start();

    expect(seen.heard).toEqual(["start"]);
    expect(seen.errors).toEqual(["network"]);
    expect(seen.ended()).toBe(1);
    expect(speechCalls()).not.toContain("plugin:speech|start");
  });

  it("asks for the microphone, and does not listen when it is denied", async () => {
    microphone = { checked: "prompt", requested: "denied" };
    const recognition = new NativeRecognition();
    const seen = watch(recognition);

    recognition.start();
    await vi.waitFor(() => expect(seen.ended()).toBe(1));

    expect(seen.errors).toEqual(["not-allowed"]);
    expect(speechCalls()).toEqual(["plugin:speech|checkPermissions", "plugin:speech|requestPermissions"]);
  });
});
