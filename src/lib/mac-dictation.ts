/**
 * Speech recognition by the macOS app (`src-tauri/src/dictation/`): WKWebView has no working
 * Web Speech recognition, so the app listens with `SFSpeechRecognizer`. `MacRecognition` has
 * the shape of the browser's recognition, so dictation (`src/lib/voice.ts`) drives either the
 * same way.
 */
import { commands, events, type DictationChanged, type DictationError, type PrivacyPane } from "@/lib/bindings";
import type { Recognition } from "@/lib/voice";

/** Error codes of the app's recognizer, in the browser's terms where there is one. */
const nativeErrorCodes: Record<DictationError["kind"], string> = {
  speechNotAllowed: "speech-not-allowed",
  microphoneNotAllowed: "microphone-not-allowed",
  noMicrophone: "audio-capture",
  languageNotSupported: "language-not-supported",
  noSpeech: "no-speech",
  failed: "native-failed",
};

/** The privacy setting that gives back what an error code says was denied. */
export const privacyPanes: Record<string, PrivacyPane> = {
  "speech-not-allowed": "speechRecognition",
  "microphone-not-allowed": "microphone",
};

/** How long a stopped session of the app's recognizer may take for its last words. */
export const NATIVE_STOP_MS = 5000;

// Session ids only need to differ from the ones before them, also across a reload.
let nativeSession = (Date.now() % 1_000_000) * 1000;

/**
 * The app's own recognizer (`src-tauri/src/dictation/`) behind the shape of the browser's:
 * the app sends the whole text heard so far with each change, and one last change when the
 * session ends.
 */
export class MacRecognition implements Recognition {
  lang = "";
  interimResults = true;
  continuous = true;
  maxAlternatives = 1;
  onresult: Recognition["onresult"] = null;
  onerror: Recognition["onerror"] = null;
  onend: Recognition["onend"] = null;
  private session = 0;
  private ended = false;
  private unlisten: (() => void) | null = null;
  private stopTimer: ReturnType<typeof setTimeout> | undefined;

  start() {
    nativeSession += 1;
    const session = nativeSession;
    this.session = session;
    // Listen first: the session's first change can come before the start command returns.
    void events.dictationChanged
      .listen(({ payload }) => this.receive(payload))
      .then(async (unlisten) => {
        if (this.ended) return unlisten();
        this.unlisten = unlisten;
        const result = await commands.startDictation(session, this.lang);
        if (result.status === "error") this.end({ kind: "failed", message: result.error.message });
      })
      .catch((cause: unknown) => this.end({ kind: "failed", message: String(cause) }));
  }

  stop() {
    if (this.ended) return;
    void commands.stopDictation(this.session);
    // The last words normally end the session; do not wait for them forever.
    this.stopTimer ??= setTimeout(() => this.end(null), NATIVE_STOP_MS);
  }

  abort() {
    if (this.ended) return;
    this.onerror?.({ error: "aborted" });
    void commands.cancelDictation(this.session);
    this.end(null);
  }

  private receive(change: DictationChanged) {
    if (this.ended || change.session !== this.session) return;
    if (change.transcript) {
      this.onresult?.({ results: [{ isFinal: change.done, 0: { transcript: change.transcript } }] });
    }
    if (change.done) this.end(change.error);
  }

  private end(error: DictationError | null) {
    if (this.ended) return;
    this.ended = true;
    clearTimeout(this.stopTimer);
    this.unlisten?.();
    if (error) this.onerror?.({ error: nativeErrorCodes[error.kind], message: error.message });
    this.onend?.();
  }
}

/** Opens the system settings where the user can allow what dictation was denied. */
export async function openPrivacySettings(pane: PrivacyPane) {
  const result = await commands.openPrivacySettings(pane);
  if (result.status === "error") throw new Error(result.error.message);
}
