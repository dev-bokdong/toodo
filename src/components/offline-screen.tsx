import { useSyncExternalStore, type ReactNode } from "react";
import { WifiOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { connectedServer } from "@/lib/platform";
import { serverName, switchServer, useConnection } from "@/lib/servers";
import { reachability, retryNow, subscribeReachability, unreachableCause } from "@/web/connection";

/**
 * Tells while `toodo-server` does not answer and takes the banner away once it answers
 * again. The app stays mounted underneath, so what was loaded stays readable and nothing
 * typed or selected is lost; buttons that change data are disabled meanwhile (see
 * `src/components/ui/button.tsx`).
 */
export function ConnectionGate({ children }: { children: ReactNode }) {
  const state = useSyncExternalStore(subscribeReachability, reachability);
  const offline = state === "unreachable" || state === "retrying";
  return (
    <>
      {offline && <OfflineBanner retrying={state === "retrying"} />}
      {children}
      {/* Room to scroll the end of the page out from under the banner, e.g. the service token fields. */}
      {offline && <div aria-hidden className="h-40" />}
    </>
  );
}

function OfflineBanner({ retrying }: { retrying: boolean }) {
  const reason = navigator.onLine ? unreachableCause() : "This device is offline.";
  return (
    <div
      role="alert"
      className="fixed inset-x-2 bottom-2 z-50 mx-auto flex max-w-xl flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-amber-500 bg-amber-100 p-3 text-sm text-amber-950 shadow-md dark:bg-amber-950 dark:text-amber-100"
    >
      <WifiOff aria-hidden className="size-4 shrink-0" />
      <div className="min-w-0 flex-1">
        <p className="font-medium">
          {connectedServer === null ? "Can't reach the Toodo server" : <ServerUnreachable origin={connectedServer} />}
        </p>
        <p className="text-xs">
          {reason} What you see may be out of date, and changes are paused. Toodo reconnects on its own as soon as
          the server answers.
        </p>
      </div>
      <Button size="sm" variant="outline" disabled={retrying} onClick={retryNow}>
        {retrying ? "Trying..." : "Retry now"}
      </Button>
      {connectedServer !== null && <OtherServers origin={connectedServer} />}
    </div>
  );
}

function ServerUnreachable({ origin }: { origin: string }) {
  const connection = useConnection();
  return <>Can't reach {serverName(connection.data, origin)}</>;
}

/** The desktop app's other saved servers, which may answer while this one does not. */
function OtherServers({ origin }: { origin: string }) {
  const connection = useConnection();
  const others = connection.data?.servers.filter((saved) => saved.url !== origin) ?? [];
  if (others.length === 0) return null;
  return (
    <div role="group" aria-label="Other servers" className="flex w-full flex-wrap gap-2">
      {others.map((saved) => (
        <Button key={saved.url} size="sm" variant="outline" title={saved.url} onClick={() =>
            switchServer(saved.url).catch((error: unknown) => console.error("Toodo: could not switch servers", error))
          }
        >
          Switch to {saved.name}
        </Button>
      ))}
    </div>
  );
}
