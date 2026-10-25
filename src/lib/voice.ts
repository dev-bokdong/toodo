/**
 * Voice in the chat with the lead, all done by the browser: dictation through the Web Speech
 * API's speech recognition, and replies read aloud through speech synthesis. The settings
 * (read replies aloud, the speech language) are kept per browser in local storage.
 *
 * See docs/voice.md for why the browser does both rather than the server. The Android app,
 * whose WebView has no speech recognition, dictates with the phone's (see docs/android.md),
 * and the macOS app with the Mac's (see docs/macos.md).
 */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { PrivacyPane } from "@/lib/bindings";
import { MacRecognition, privacyPanes } from "@/lib/mac-dictation";
import { NativeRecognition, withNativeFallback } from "@/lib/native-speech";
import { mobileApp, nativeDictation } from "@/lib/platform";

const READ_ALOUD_KEY = "toodo-read-aloud";
const LANGUAGE_KEY = "toodo-speech-language";
const listeners = new Set<() => void>();

function notify() {
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  const onStorage = (event: StorageEvent) => {
    if (event.key === READ_ALOUD_KEY || event.key === LANGUAGE_KEY) listener();
  };
  window.addEventListener("storage", onStorage);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("storage", onStorage);
  };
}

export const speechLanguages: { tag: string; label: string }[] = [
  { tag: "ko-KR", label: "Korean" },
  { tag: "en-US", label: "English (US)" },
];

export function getReadAloud(): boolean {
  return localStorage.getItem(READ_ALOUD_KEY) === "on";
}

export function setReadAloud(on: boolean) {
  localStorage.setItem(READ_ALOUD_KEY, on ? "on" : "off");
  if (!on) stopSpeaking();
  notify();
}

export function useReadAloud(): boolean {
  return useSyncExternalStore(subscribe, getReadAloud);
}

export function getSpeechLanguage(): string {
  return localStorage.getItem(LANGUAGE_KEY) || navigator.language || "en-US";
}

export function setSpeechLanguage(tag: string) {
  localStorage.setItem(LANGUAGE_KEY, tag);
  notify();
}

export function useSpeechLanguage(): string {
  return useSyncExternalStore(subscribe, getSpeechLanguage);
}

// Speech recognition is still prefixed in Chrome, Samsung Internet and Safari, and missing in
// Firefox; its types are not in the DOM library yet.
type RecognitionResult = { readonly isFinal: boolean; readonly 0: { readonly transcript: string } };
type RecognitionEvent = { readonly results: ArrayLike<RecognitionResult> };
type RecognitionErrorEvent = { readonly error: string; readonly message?: string };
export type Recognition = {
  lang: string;
  interimResults: boolean;
  continuous: boolean;
  maxAlternatives: number;
  onresult: ((event: RecognitionEvent) => void) | null;
  onerror: ((event: RecognitionErrorEvent) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
};
type RecognitionConstructor = new () => Recognition;

function recognitionConstructor(): RecognitionConstructor | null {
  if (nativeDictation) return MacRecognition;
  const scope = window as unknown as {
    SpeechRecognition?: RecognitionConstructor;
    webkitSpeechRecognition?: RecognitionConstructor;
  };
  const web = scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null;
  if (!mobileApp) return web;
  return web === null ? NativeRecognition : withNativeFallback(web);
}

export function dictationSupported(): boolean {
  return recognitionConstructor() !== null;
}

const recognitionErrors: Record<string, string> = {
  "not-allowed": mobileApp
    ? "Microphone access is blocked. Allow it for Toodo in Android's app settings."
    : "Microphone access is blocked. Allow it in the browser's site settings.",
  "service-not-allowed": "This browser does not allow speech recognition here.",
  "audio-capture": "No microphone was found.",
  "no-speech": "No speech was heard. Try again.",
  network: "Speech recognition needs a network connection.",
  "language-not-supported": "Speech recognition does not support the chosen language.",
};

/**
 * How long dictation waits for more words before it stops by itself. Single-utterance
 * recognition ends at the first pause it takes for the end of the sentence, which would send
 * half a message from someone stopping to think, so dictation listens continuously and stops
 * once the speaker has been quiet this long.
 */
export const QUIET_MS = 2000;

// Dictations listening now, in any component: the app's wake word holds while one does.
let dictating = 0;
const dictatingListeners = new Set<() => void>();

function changeDictating(by: number) {
  dictating += by;
  dictatingListeners.forEach((listener) => listener());
}

function subscribeDictating(listener: () => void): () => void {
  dictatingListeners.add(listener);
  return () => dictatingListeners.delete(listener);
}

/** Whether a message is being dictated anywhere in the window. */
export function useDictating(): boolean {
  return useSyncExternalStore(subscribeDictating, () => dictating > 0);
}

export type Dictation = {
  supported: boolean;
  listening: boolean;
  error: string | null;
  /** Whether the last attempt failed because the microphone is not allowed. */
  blocked: boolean;
  /** Where in the macOS system settings the user can allow what the error says was denied. */
  privacyPane: PrivacyPane | null;
  start(): void;
  /** Stops listening and hands over what was heard. */
  stop(): void;
  /** Stops listening and drops what was heard. */
  cancel(): void;
};

/**
 * Dictation of one message: listening stops by itself after `QUIET_MS` of quiet, or on `stop`. The
 * transcript comes in pieces while the person speaks, then once whole unless cancelled.
 */
export function useDictation(onTranscript: (text: string) => void, onDone: (text: string) => void): Dictation {
  const [listening, setListening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [blocked, setBlocked] = useState(false);
  const [privacyPane, setPrivacyPane] = useState<PrivacyPane | null>(null);
  const recognition = useRef<Recognition | null>(null);
  const cancelled = useRef(false);
  const callbacks = useRef({ onTranscript, onDone });
  callbacks.current = { onTranscript, onDone };

  useEffect(() => () => recognition.current?.abort(), []);

  useEffect(() => {
    if (!listening) return;
    changeDictating(1);
    return () => changeDictating(-1);
  }, [listening]);

  const start = useCallback(() => {
    const Constructor = recognitionConstructor();
    if (Constructor === null || recognition.current !== null) return;
    const current = new Constructor();
    let transcript = "";
    let quiet: ReturnType<typeof setTimeout> | undefined;
    cancelled.current = false;
    current.lang = getSpeechLanguage();
    current.interimResults = true;
    current.continuous = true;
    current.maxAlternatives = 1;
    current.onresult = (event) => {
      transcript = Array.from(event.results, (result) => result[0].transcript).join("");
      callbacks.current.onTranscript(transcript);
      clearTimeout(quiet);
      quiet = setTimeout(() => current.stop(), QUIET_MS);
    };
    current.onerror = (event) => {
      if (event.error === "aborted") {
        cancelled.current = true;
        return;
      }
      setError(recognitionErrors[event.error] ?? event.message ?? `Speech recognition failed: ${event.error}`);
      setBlocked(event.error === "not-allowed");
      setPrivacyPane(privacyPanes[event.error] ?? null);
    };
    current.onend = () => {
      clearTimeout(quiet);
      recognition.current = null;
      setListening(false);
      if (!cancelled.current) callbacks.current.onDone(transcript);
    };
    setError(null);
    setBlocked(false);
    setPrivacyPane(null);
    recognition.current = current;
    setListening(true);
    try {
      current.start();
    } catch (cause) {
      recognition.current = null;
      setListening(false);
      setError(`Speech recognition failed: ${cause instanceof Error ? cause.message : String(cause)}`);
    }
  }, []);

  const stop = useCallback(() => recognition.current?.stop(), []);
  const cancel = useCallback(() => {
    cancelled.current = true;
    recognition.current?.abort();
  }, []);

  return { supported: recognitionConstructor() !== null, listening, error, blocked, privacyPane, start, stop, cancel };
}

/** The text of a Markdown reply as it should sound: no markup, code, or addresses. */
export function speakableText(markdown: string): string {
  return markdown
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/https?:\/\/\S+/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/^\s{0,3}(#{1,6}\s+|>\s?|[-*+]\s+|\d+[.)]\s+)/gm, "")
    .replace(/[*~]+/g, "")
    .replace(/\|/g, " ")
    .replace(/[ \t]+/g, " ")
    .replace(/\s*\n\s*/g, "\n")
    .trim();
}

/**
 * Splits text into pieces of up to `limit` characters at sentence or line ends where it can:
 * Chrome stops a long utterance after about fifteen seconds.
 */
export function speechChunks(text: string, limit = 200): string[] {
  const sentences = text.match(/[^.!?\n]+[.!?]*\s*|\n+/g) ?? [];
  const chunks: string[] = [];
  let chunk = "";
  for (const sentence of sentences) {
    for (let start = 0; start < sentence.length; start += limit) {
      const piece = sentence.slice(start, start + limit);
      if (chunk.length + piece.length > limit) {
        chunks.push(chunk);
        chunk = "";
      }
      chunk += piece;
    }
  }
  chunks.push(chunk);
  return chunks.map((piece) => piece.trim()).filter((piece) => piece.length > 0);
}

export function speechSupported(): boolean {
  return typeof window !== "undefined" && "speechSynthesis" in window;
}

// Utterances queued and not finished yet; a stop starts a new generation so the queue's late
// end events do not count against the next reply.
let pending = 0;
let generation = 0;
const speakingListeners = new Set<() => void>();

function setPending(next: number) {
  pending = Math.max(0, next);
  speakingListeners.forEach((listener) => listener());
}

function voiceFor(tag: string): SpeechSynthesisVoice | null {
  const wanted = tag.toLowerCase();
  const voices = window.speechSynthesis.getVoices();
  const sameTag = (voice: SpeechSynthesisVoice) => voice.lang.replace("_", "-").toLowerCase() === wanted;
  const sameLanguage = (voice: SpeechSynthesisVoice) =>
    voice.lang.toLowerCase().split(/[-_]/)[0] === wanted.split("-")[0];
  return voices.find(sameTag) ?? voices.find(sameLanguage) ?? null;
}

/** Reads `markdown` aloud after whatever is being read already. */
export function speak(markdown: string) {
  if (!speechSupported()) return;
  const chunks = speechChunks(speakableText(markdown));
  if (chunks.length === 0) return;
  const tag = getSpeechLanguage();
  const voice = voiceFor(tag);
  const mine = generation;
  for (const chunk of chunks) {
    const utterance = new SpeechSynthesisUtterance(chunk);
    utterance.lang = tag;
    if (voice) utterance.voice = voice;
    utterance.onend = utterance.onerror = () => {
      if (mine === generation) setPending(pending - 1);
    };
    window.speechSynthesis.speak(utterance);
  }
  setPending(pending + chunks.length);
}

export function stopSpeaking() {
  if (!speechSupported()) return;
  generation += 1;
  window.speechSynthesis.cancel();
  setPending(0);
}

function subscribeSpeaking(listener: () => void): () => void {
  speakingListeners.add(listener);
  return () => speakingListeners.delete(listener);
}

export function useSpeaking(): boolean {
  return useSyncExternalStore(subscribeSpeaking, () => pending > 0);
}
