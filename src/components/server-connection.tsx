import { useId, useState, type FormEvent } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import { commands } from "@/lib/bindings";
import { connectedServer } from "@/lib/platform";

/**
 * Where the desktop app's data lives: on this computer, or on a `toodo-server` it connects
 * to as one of the server's clients. Switching restarts the app, which then starts its own
 * data and background work, or none of it and talks to the server.
 */
export function ServerConnection() {
  const [address, setAddress] = useState(connectedServer ?? "");
  const [error, setError] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);
  const id = useId();

  async function connect(server: string | null) {
    setSwitching(true);
    setError(null);
    const result = await commands.setConnection(server);
    if (result.status === "error") {
      setError(result.error.message);
      setSwitching(false);
      return;
    }
    await relaunch();
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void connect(address.trim());
  }

  const changed = address.trim() !== "" && address.trim() !== connectedServer;
  return (
    <section aria-label="Data" className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">Data</span>
        <span className="text-muted-foreground">
          {connectedServer === null ? "On this computer" : `On the server ${connectedServer}`}
        </span>
      </div>
      <form onSubmit={submit} aria-label="Connect to a Toodo server" className="flex flex-col gap-1">
        <label htmlFor={`${id}-address`} className="text-xs text-muted-foreground">
          Toodo server address
        </label>
        <div className="flex flex-wrap gap-2">
          <input
            id={`${id}-address`}
            className={`${inputClass} min-w-0 flex-1`}
            type="url"
            inputMode="url"
            placeholder="https://toodo.example"
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            aria-describedby={`${id}-help`}
          />
          <Button type="submit" size="sm" disabled={switching || !changed}>
            {connectedServer === null ? "Connect" : "Switch"}
          </Button>
        </div>
        <p id={`${id}-help`} className="text-xs text-muted-foreground">
          The app restarts and shows the server's data instead of this computer's, which stays as it is.
        </p>
      </form>
      {connectedServer !== null && (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          disabled={switching}
          onClick={() => void connect(null)}
        >
          Use this computer's data
        </Button>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}
