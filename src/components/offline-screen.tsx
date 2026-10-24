import { useSyncExternalStore, type ReactNode } from "react";
import { WifiOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { reachability, retryNow, subscribeReachability } from "@/web/connection";

/**
 * Covers the app with the offline screen while `toodo-server` does not answer, and takes it
 * away once the server answers again. The app stays mounted underneath, so nothing typed or
 * selected is lost.
 */
export function ConnectionGate({ children }: { children: ReactNode }) {
  const state = useSyncExternalStore(subscribeReachability, reachability);
  const offline = state === "unreachable" || state === "retrying";
  return (
    <>
      <div className="contents" inert={offline}>
        {children}
      </div>
      {offline && <OfflineScreen retrying={state === "retrying"} />}
    </>
  );
}

function OfflineScreen({ retrying }: { retrying: boolean }) {
  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="offline-title"
      aria-describedby="offline-description"
      className="fixed inset-0 z-50 flex items-center justify-center bg-background p-6 text-foreground"
    >
      <div className="flex w-full max-w-sm flex-col items-center gap-3 text-center">
        <WifiOff aria-hidden className="size-8 text-muted-foreground" />
        <h2 id="offline-title" className="text-lg font-semibold">
          Can't reach the Toodo server
        </h2>
        <p id="offline-description" className="text-sm text-muted-foreground">
          {navigator.onLine ? "The server or the connection to it is down." : "This device is offline."} Toodo
          reconnects on its own as soon as the server answers.
        </p>
        <div className="flex gap-2">
          <Button autoFocus disabled={retrying} onClick={retryNow}>
            {retrying ? "Trying..." : "Try again"}
          </Button>
          <Button variant="outline" onClick={() => location.reload()}>
            Reload
          </Button>
        </div>
      </div>
    </div>
  );
}
