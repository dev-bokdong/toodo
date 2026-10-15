import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { Button } from "@/components/ui/button";
import { tonePanel } from "@/lib/status-styles";
import { cn } from "@/lib/utils";

const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch (error) {
    // Being offline or GitHub being unreachable is routine; the app keeps working on
    // the installed version, so log the failure instead of interrupting the user.
    console.warn("Update check failed:", error);
    return null;
  }
}

export function UpdateBanner() {
  const update = useQuery({
    queryKey: ["app-update"],
    queryFn: checkForUpdate,
    refetchInterval: CHECK_INTERVAL_MS,
    // Closing the window hides the app to the tray, which can leave the document hidden
    // for days; keep checking so the update is waiting when the window comes back.
    refetchIntervalInBackground: true,
    refetchOnWindowFocus: false,
  });
  const [progress, setProgress] = useState<number | null>(null);
  const install = useMutation({
    mutationFn: async (available: Update) => {
      let total = 0;
      let received = 0;
      await available.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
          setProgress(0);
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          setProgress(total > 0 ? Math.min(100, Math.round((received / total) * 100)) : null);
        }
      });
      await relaunch();
    },
    onSettled: () => setProgress(null),
  });

  const available = update.data;
  if (!available) {
    return null;
  }

  return (
    <div
      role="status"
      aria-label="App update"
      className={cn("mb-4 flex items-center justify-between gap-3 rounded-lg px-3 py-2", tonePanel.info)}
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="text-sm font-medium">Toodo {available.version} is available</p>
        <p className="text-xs break-words">
          {install.isError
            ? `Update failed: ${install.error instanceof Error ? install.error.message : String(install.error)}`
            : `You are running ${available.currentVersion}. Installing restarts Toodo.`}
        </p>
      </div>
      <Button size="sm" variant="outline" onClick={() => install.mutate(available)} disabled={install.isPending}>
        {install.isPending
          ? progress === null
            ? "Installing..."
            : `Downloading ${progress}%`
          : "Install and restart"}
      </Button>
    </div>
  );
}
