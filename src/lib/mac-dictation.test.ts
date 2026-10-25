import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DictationChanged } from "@/lib/bindings";

const app = vi.hoisted(() => ({
  handler: null as ((event: { payload: DictationChanged }) => void) | null,
  startDictation: vi.fn(async (_session: number, _language: string) => ({ status: "ok" as const, data: null })),
  stopDictation: vi.fn(async (_session: number) => ({ status: "ok" as const, data: null })),
  cancelDictation: vi.fn(async (_session: number) => ({ status: "ok" as const, data: null })),
  openPrivacySettings: vi.fn(async (_pane: string) => ({ status: "ok" as const, data: null })),
  unlisten: vi.fn(),
}));

vi.mock("@/lib/platform", async (original) => ({
  ...await original<typeof import("@/lib/platform")>(),
  nativeDictation: true,
  mobileApp: false,
}));
vi.mock("@/lib/bindings", () => ({
  commands: {
    startDictation: app.startDictation,
    stopDictation: app.stopDictation,
    cancelDictation: app.cancelDictation,
    openPrivacySettings: app.openPrivacySettings,
  },
  events: {
    dictationChanged: {
      listen: async (handler: (event: { payload: DictationChanged }) => void) => {
        app.handler = handler;
        return app.unlisten;
      },
    },
  },
}));

const { NATIVE_STOP_MS, MacRecognition } = await import("@/lib/mac-dictation");
const { useDictation } = await import("@/lib/voice");

function changeLastSession(transcript: string, done = false, error: DictationChanged["error"] = null) {
  const session = app.startDictation.mock.lastCall![0] as number;
  app.handler!({ payload: { session, transcript, done, error } });
}

async function started(recognition: InstanceType<typeof MacRecognition>) {
  recognition.lang = "ko-KR";
  recognition.start();
  await vi.waitFor(() => expect(app.startDictation).toHaveBeenCalled());
}

beforeEach(() => {
  vi.clearAllMocks();
  app.handler = null;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("MacRecognition", () => {
  it("starts the app's recognizer in the language and reports what it hears until it ends", async () => {
    const recognition = new MacRecognition();
    const heard: string[] = [];
    const ended = vi.fn();
    recognition.onresult = (event) => heard.push(event.results[0][0].transcript);
    recognition.onend = ended;
    await started(recognition);
    expect(app.startDictation).toHaveBeenCalledWith(expect.any(Number), "ko-KR");

    changeLastSession("\uC624\uB298");
    changeLastSession("\uC624\uB298 \uD560 \uC77C");
    expect(ended).not.toHaveBeenCalled();
    changeLastSession("\uC624\uB298 \uD560 \uC77C \uBCF4\uC5EC \uC918", true);
    changeLastSession("late", true);

    expect(heard).toEqual(["\uC624\uB298", "\uC624\uB298 \uD560 \uC77C", "\uC624\uB298 \uD560 \uC77C \uBCF4\uC5EC \uC918"]);
    expect(ended).toHaveBeenCalledTimes(1);
    expect(app.unlisten).toHaveBeenCalled();
  });

  it("ignores the changes of other sessions", async () => {
    const recognition = new MacRecognition();
    const onresult = vi.fn();
    recognition.onresult = onresult;
    await started(recognition);
    const session = app.startDictation.mock.lastCall![0] as number;
    app.handler!({ payload: { session: session + 1, transcript: "other", done: false, error: null } });
    expect(onresult).not.toHaveBeenCalled();
  });

  it("reports a denied permission with the app's message", async () => {
    const recognition = new MacRecognition();
    const onerror = vi.fn();
    const onend = vi.fn();
    recognition.onerror = onerror;
    recognition.onend = onend;
    await started(recognition);
    changeLastSession("", true, { kind: "microphoneNotAllowed", message: "Allow the microphone." });
    expect(onerror).toHaveBeenCalledWith({ error: "microphone-not-allowed", message: "Allow the microphone." });
    expect(onend).toHaveBeenCalledTimes(1);
  });

  it("ends a stopped session whose last words never come", async () => {
    const recognition = new MacRecognition();
    const onend = vi.fn();
    recognition.onend = onend;
    await started(recognition);
    vi.useFakeTimers();
    recognition.stop();
    expect(app.stopDictation).toHaveBeenCalled();
    vi.advanceTimersByTime(NATIVE_STOP_MS - 1);
    expect(onend).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onend).toHaveBeenCalledTimes(1);
  });

  it("cancels in the app on abort", async () => {
    const recognition = new MacRecognition();
    const onerror = vi.fn();
    const onend = vi.fn();
    recognition.onerror = onerror;
    recognition.onend = onend;
    await started(recognition);
    recognition.abort();
    expect(app.cancelDictation).toHaveBeenCalledWith(app.startDictation.mock.lastCall![0]);
    expect(onerror).toHaveBeenCalledWith({ error: "aborted" });
    expect(onend).toHaveBeenCalledTimes(1);
  });

  it("fails the session when the app cannot start it", async () => {
    app.startDictation.mockResolvedValueOnce({
      status: "error",
      error: { kind: "internal", message: "no main thread" },
    } as never);
    const recognition = new MacRecognition();
    const onerror = vi.fn();
    recognition.onerror = onerror;
    await started(recognition);
    await vi.waitFor(() =>
      expect(onerror).toHaveBeenCalledWith({ error: "native-failed", message: "no main thread" }),
    );
  });
});

describe("useDictation in the macOS app", () => {
  it("uses the app's recognizer and hands over the whole message", async () => {
    const onTranscript = vi.fn();
    const onDone = vi.fn();
    const { result } = renderHook(() => useDictation(onTranscript, onDone));
    expect(result.current.supported).toBe(true);
    act(() => result.current.start());
    await vi.waitFor(() => expect(app.startDictation).toHaveBeenCalled());
    act(() => changeLastSession("\uB3C4\uB9AC\uC57C"));
    expect(onTranscript).toHaveBeenLastCalledWith("\uB3C4\uB9AC\uC57C");
    act(() => changeLastSession("\uB3C4\uB9AC\uC57C \uC548\uB155", true));
    expect(onDone).toHaveBeenCalledWith("\uB3C4\uB9AC\uC57C \uC548\uB155");
    expect(result.current.listening).toBe(false);
  });

  it("points a denied permission to its privacy settings", async () => {
    const { result } = renderHook(() => useDictation(vi.fn(), vi.fn()));
    act(() => result.current.start());
    await vi.waitFor(() => expect(app.startDictation).toHaveBeenCalled());
    act(() => changeLastSession("", true, { kind: "speechNotAllowed", message: "Allow speech recognition." }));
    expect(result.current.error).toBe("Allow speech recognition.");
    expect(result.current.privacyPane).toBe("speechRecognition");
  });

  it("keeps the browser's message for errors both share, without a settings link", async () => {
    const { result } = renderHook(() => useDictation(vi.fn(), vi.fn()));
    act(() => result.current.start());
    await vi.waitFor(() => expect(app.startDictation).toHaveBeenCalled());
    act(() => changeLastSession("", true, { kind: "noSpeech", message: "No speech was heard. Try again." }));
    expect(result.current.error).toBe("No speech was heard. Try again.");
    expect(result.current.privacyPane).toBeNull();
  });
});
