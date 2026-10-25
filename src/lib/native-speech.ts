/**
 * Speech recognition by the phone, for the Android app: its WebView has no Web Speech API, so
 * the app's speech plugin (`SpeechPlugin.kt` in the Android project) runs Android's
 * `SpeechRecognizer`. `NativeRecognition` has the shape of the browser's recognition, so
 * dictation (`src/lib/voice.ts`) drives either the same way.
 */
import { addPluginListener, invoke, type PluginListener } from "@tauri-apps/api/core";

type PermissionState = "granted" | "denied" | "prompt" | "prompt-with-rationale";
type Permissions = { microphone: PermissionState };

type ResultEvent = { readonly results: ArrayLike<{ readonly isFinal: boolean; readonly 0: { readonly transcript: string } }> };

/** Opens the app's page in Android's settings, where a microphone denied for good is allowed again. */
export function openAppSettings(): Promise<void> {
  return invoke("plugin:speech|openSettings");
}

async function microphoneAllowed(): Promise<boolean> {
  const { microphone } = await invoke<Permissions>("plugin:speech|checkPermissions");
  if (microphone === "granted") return true;
  // Android asks again until the person denies twice; after that the answer is "denied" at once.
  const asked = await invoke<Permissions>("plugin:speech|requestPermissions", { permissions: ["microphone"] });
  return asked.microphone === "granted";
}

export class NativeRecognition {
  lang = "";
  interimResults = false;
  continuous = false;
  maxAlternatives = 1;
  onresult: ((event: ResultEvent) => void) | null = null;
  onerror: ((event: { readonly error: string }) => void) | null = null;
  onend: (() => void) | null = null;
  private aborted = false;
  private listeners: PluginListener[] = [];

  start() {
    void this.run().catch((cause: unknown) => {
      this.end(cause instanceof Error ? cause.message : String(cause));
    });
  }

  stop() {
    void invoke("plugin:speech|stop");
  }

  abort() {
    this.aborted = true;
    void invoke("plugin:speech|cancel");
  }

  private async run() {
    // The microphone is asked for first, so dictation opened from a link waits for the answer.
    if (!(await microphoneAllowed())) return this.end("not-allowed");
    if (this.aborted) return this.end("aborted");
    this.listeners = await Promise.all([
      addPluginListener<{ text: string; final: boolean }>("speech", "result", ({ text, final }) =>
        this.onresult?.({ results: [{ isFinal: final, 0: { transcript: text } }] }),
      ),
      addPluginListener<{ error: string }>("speech", "error", ({ error }) => this.onerror?.({ error })),
      addPluginListener("speech", "end", () => this.end(null)),
    ]);
    await invoke("plugin:speech|start", { lang: this.lang, partialResults: this.interimResults });
  }

  private end(error: string | null) {
    if (error !== null) this.onerror?.({ error });
    const listeners = this.listeners;
    this.listeners = [];
    listeners.forEach((listener) => void listener.unregister());
    this.onend?.();
  }
}
