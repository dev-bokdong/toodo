import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { openAppSettings } from "@/lib/native-speech";
import { useSendChatMessage } from "@/lib/queries";
import {
  cancelWake,
  connectWakeWord,
  retryWakeMessage,
  setWakeEnabled,
  useWakeEnabled,
  useWakeState,
} from "@/lib/wake-word";
import { activeServer } from "@/web/server";

export function WakeWordSettings() {
  const enabled = useWakeEnabled();
  const state = useWakeState();
  const [changing, setChanging] = useState(false);
  return (
    <div className="flex flex-col gap-2">
      <label className="flex min-h-11 items-center gap-2">
        <input
          type="checkbox"
          checked={enabled}
          disabled={changing}
          aria-describedby="wake-word-help"
          onChange={(event) => {
            setChanging(true);
            void setWakeEnabled(event.target.checked).finally(() => setChanging(false));
          }}
        />
        Listen for Dori (do-ri-ya)
      </label>
      <p id="wake-word-help" className="text-xs text-muted-foreground">
        Off by default. While Toodo is open, say the Korean wake word followed by your message.
        Recognition stays on this phone; only the completed message is sent. Listening pauses
        when you leave, lock the screen, dictate, or play a reply aloud. Uses more battery.
        The Korean recognition model downloads on first use.
      </p>
      {enabled && <p role="status">{stateNames[state.state]}</p>}
      {state.error && (
        <div className="flex flex-col items-start gap-2">
          <p role="alert" className="text-sm text-destructive">{state.error}</p>
          <Button type="button" variant="outline" onClick={() => void openAppSettings()}>
            Open app settings
          </Button>
        </div>
      )}
    </div>
  );
}

const stateNames = {
  off: "Wake word off",
  preparing: "Preparing Korean recognition model",
  listening: "Listening for Dori",
  capturing: "Listening to your message",
  paused: "Wake word paused",
  pending: "Voice message saved on this phone",
  error: "Wake word unavailable",
} as const;

/** Kept mounted with the chat so hands-free input also works while the panel is closed. */
export function WakeWordChat() {
  const { mutateAsync } = useSendChatMessage();
  const enabled = useWakeEnabled();
  const state = useWakeState();
  useEffect(
    () => connectWakeWord(({ text, id }) => mutateAsync({ text, clientId: id })),
    [mutateAsync],
  );
  useEffect(() => {
    if (state.state !== "capturing") return;
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      cancelWake();
    };
    document.addEventListener("keydown", cancel, true);
    return () => document.removeEventListener("keydown", cancel, true);
  }, [state.state]);
  if (!enabled && !state.pending && !state.error) return null;
  const differentServer = state.pending !== null && state.pending.scope !== activeServer.id;
  return (
    <div className="flex flex-col gap-2 border-b px-4 py-2 text-sm">
      <p role="status">{state.sending ? "Sending voice message" : stateNames[state.state]}</p>
      {state.state === "capturing" && (
        <Button type="button" variant="outline" onClick={cancelWake}>
          Cancel voice message
        </Button>
      )}
      {state.error && <p role="alert" className="text-destructive">{state.error}</p>}
      {state.pending && !state.sending && (
        <>
          <p className="max-h-32 overflow-y-auto break-words-keep">{state.pending.text}</p>
          {differentServer ? (
            <p className="break-all text-muted-foreground">
              Switch back to {state.pending.scope} to send this saved message.
            </p>
          ) : (
            <>
              {state.sendError && <p role="alert" className="text-destructive">{state.sendError}</p>}
              <Button type="button" variant="outline" onClick={() => void retryWakeMessage()}>
                Retry voice message
              </Button>
            </>
          )}
        </>
      )}
    </div>
  );
}
