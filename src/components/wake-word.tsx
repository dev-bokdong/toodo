/**
 * The wake word in the window of the macOS and Windows apps: follows what the app hears,
 * holds detection while a reply is read aloud or a message is dictated, and shows what is
 * heard after "Dori-ya" until the sentence ends. A click on the indicator, or Escape, drops
 * the sentence.
 */
import { useEffect, useState } from "react";
import { Mic, XIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { WakeWordStatus } from "@/lib/bindings";
import { openPrivacySettings } from "@/lib/mac-dictation";
import { useNarrow } from "@/lib/narrow";
import { appPlatform } from "@/lib/platform";
import { cn } from "@/lib/utils";
import { useDictating, useSpeaking } from "@/lib/voice";
import {
  cancelWakeWord,
  followWakeWord,
  setWakeWordDictating,
  setWakeWordEnabled,
  setWakeWordSpeaking,
  useWakeWord,
} from "@/lib/wake-word";

const onWindows = appPlatform === "windows";
/** What the app's privacy settings are called, and the computer it runs on. */
const systemSettings = onWindows ? "Windows Settings" : "System Settings";
const computer = onWindows ? "this PC" : "this Mac";
const hiddenIn = onWindows ? "in the tray" : "closed";

/** How many characters of a long sentence the indicator shows while it is heard. */
const TAIL = { narrow: 16, wide: 32 };

export function WakeWord() {
  const wake = useWakeWord();
  const speaking = useSpeaking();
  const dictating = useDictating();
  const hearing = wake.status.state === "hearing" ? wake.status.transcript : null;
  const isHearing = hearing !== null;
  const tail = useNarrow() ? TAIL.narrow : TAIL.wide;

  useEffect(() => followWakeWord(), []);

  useEffect(() => {
    if (wake.enabled) setWakeWordSpeaking(speaking);
  }, [wake.enabled, speaking]);

  useEffect(() => {
    if (wake.enabled) setWakeWordDictating(dictating);
  }, [wake.enabled, dictating]);

  useEffect(() => {
    if (!isHearing) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) {
        event.preventDefault();
        cancelWakeWord();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [isHearing]);

  if (hearing === null) return null;
  return (
    <div
      role="status"
      aria-label="Voice message"
      className="fixed top-[calc(0.75rem+env(safe-area-inset-top))] left-1/2 z-50 flex max-w-[min(32rem,calc(100vw-2rem))] -translate-x-1/2 items-center gap-2 rounded-full border bg-card py-1.5 pr-1.5 pl-3 text-sm shadow-lg"
    >
      <Mic aria-hidden className="size-4 shrink-0 text-destructive motion-safe:animate-pulse" />
      <span className="min-w-0 truncate break-words-keep">
        {hearing ? (
          // The newest words matter most while speaking; a long sentence shows its end.
          hearing.length > tail ? `\u2026${hearing.slice(-tail)}` : hearing
        ) : (
          <span className="text-muted-foreground">Listening. Say your message.</span>
        )}
      </span>
      <button
        type="button"
        aria-label="Cancel voice message"
        title="Cancel (Esc)"
        onClick={cancelWakeWord}
        className="flex size-7 shrink-0 items-center justify-center rounded-full text-muted-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring max-md:size-11"
      >
        <XIcon className="size-4" />
      </button>
    </div>
  );
}

function megabytes(bytes: number | null): string {
  return `${Math.round((bytes ?? 0) / 1_000_000)} MB`;
}

function statusText(status: WakeWordStatus): string {
  switch (status.state) {
    case "off":
      return "Off";
    case "downloading":
      return `Downloading the speech model: ${megabytes(status.doneBytes)} of ${megabytes(status.totalBytes)}`;
    case "starting":
      return "Starting";
    case "listening":
      return 'Listening for "Dori-ya"';
    case "held":
      return "Paused while a reply is read aloud or a message is dictated";
    case "hearing":
      return "Hearing a message";
    case "failed":
      return status.error.message;
  }
}

/** The wake word switch in Settings > General, with what the app is doing about it. */
export function WakeWordSetting() {
  const wake = useWakeWord();
  const [error, setError] = useState<string | null>(null);
  const failed = wake.status.state === "failed" ? wake.status.error : null;

  function change(enabled: boolean) {
    setError(null);
    setWakeWordEnabled(enabled).catch((cause: unknown) => setError(String(cause)));
  }

  return (
    <div className="flex flex-col gap-1.5">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={wake.enabled} onChange={(event) => change(event.target.checked)} />
        Send a message by saying "Dori-ya" and then the message
      </label>
      <p
        role="status"
        aria-label="Wake word status"
        className={cn("text-xs", failed ? "text-destructive" : "text-muted-foreground")}
      >
        {statusText(wake.status)}
      </p>
      {failed?.kind === "microphoneNotAllowed" && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          changesData={false}
          className="self-start"
          onClick={() => void openPrivacySettings("microphone").catch(console.error)}
        >
          Open {systemSettings}
        </Button>
      )}
      {wake.enabled && failed !== null && failed.kind !== "microphoneNotAllowed" && (
        <Button
          type="button"
          size="sm"
          variant="outline"
          changesData={false}
          className="self-start"
          onClick={() => change(true)}
        >
          Try again
        </Button>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      <p className="text-xs text-muted-foreground">
        Toodo listens on {computer} only while it runs, also with its window {hiddenIn}, and stops on Quit. The first
        time, it downloads its speech model (about 140 MB); what you say is recognized on {computer}.
      </p>
    </div>
  );
}
