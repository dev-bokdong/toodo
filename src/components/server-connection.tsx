import { useEffect, useId, useState, type FormEvent } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import { commands, type AccessTokenInfo } from "@/lib/bindings";
import { connectedServer } from "@/lib/platform";
import { retryNow } from "@/web/connection";

/**
 * Where the desktop app's data lives: on this computer, or on a `toodo-server` it connects
 * to as one of the server's clients. Switching restarts the app, which then starts its own
 * data and background work, or none of it and talks to the server. A server behind
 * Cloudflare Access also needs an Access service token, which the app keeps and sends with
 * every request to that server; it never shows the secret again.
 */
export function ServerConnection() {
  const [address, setAddress] = useState(connectedServer ?? "");
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [kept, setKept] = useState<AccessTokenInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);
  const id = useId();

  useEffect(() => {
    if (connectedServer === null) return;
    void commands.getAccessToken(connectedServer).then((result) => {
      if (result.status === "ok") setKept(result.data);
    });
  }, []);

  const server = address.trim();
  const tokenEntered = clientId.trim() !== "" || clientSecret.trim() !== "";
  const sameServer = server === connectedServer;

  async function saveToken(): Promise<boolean> {
    if (!tokenEntered) return true;
    const result = await commands.setAccessToken(server, { clientId, clientSecret });
    if (result.status === "error") {
      setError(result.error.message);
      return false;
    }
    setClientId("");
    setClientSecret("");
    return true;
  }

  async function connect(target: string | null) {
    setSwitching(true);
    setError(null);
    setNotice(null);
    if (target !== null && !(await saveToken())) {
      setSwitching(false);
      return;
    }
    if (target !== null && target === connectedServer) {
      // Only the token changed: the app sends it from the next request on.
      const result = await commands.getAccessToken(target);
      if (result.status === "ok") setKept(result.data);
      setNotice("Saved. The app sends the new service token from now on.");
      setSwitching(false);
      retryNow();
      return;
    }
    const result = await commands.setConnection(target);
    if (result.status === "error") {
      setError(result.error.message);
      setSwitching(false);
      return;
    }
    await relaunch();
  }

  async function removeToken() {
    if (connectedServer === null) return;
    setError(null);
    setNotice(null);
    const result = await commands.setAccessToken(connectedServer, null);
    if (result.status === "error") {
      setError(result.error.message);
      return;
    }
    setKept(null);
    setNotice("Removed the service token.");
  }

  function run(target: string | null) {
    connect(target).catch((failure: unknown) => {
      setError(failure instanceof Error ? failure.message : String(failure));
      setSwitching(false);
    });
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    run(server);
  }

  const changed = server !== "" && (!sameServer || tokenEntered);
  return (
    <section aria-label="Data" className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">Data</span>
        <span className="text-muted-foreground">
          {connectedServer === null ? "On this computer" : `On the server ${connectedServer}`}
        </span>
      </div>
      <form onSubmit={submit} aria-label="Connect to a Toodo server" className="flex flex-col gap-2">
        <div className="flex flex-col gap-1">
          <label htmlFor={`${id}-address`} className="text-xs text-muted-foreground">
            Toodo server address
          </label>
          <input
            id={`${id}-address`}
            className={`${inputClass} min-w-0`}
            type="url"
            inputMode="url"
            placeholder="https://toodo.example"
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            aria-describedby={`${id}-help`}
          />
          <p id={`${id}-help`} className="text-xs text-muted-foreground">
            The app restarts and shows the server's data instead of this computer's, which stays as it is.
          </p>
        </div>
        <fieldset className="flex flex-col gap-1 rounded-md border p-2" aria-describedby={`${id}-access-help`}>
          <legend className="px-1 text-xs font-medium">Cloudflare Access service token</legend>
          <p id={`${id}-access-help`} className="text-xs text-muted-foreground">
            Only for a server behind Cloudflare Access. The app sends the token with every request to the server;
            the secret is kept in the app and never shown again.
          </p>
          {sameServer && kept !== null && (
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
              <span>
                In use: <span className="font-mono break-all">{kept.clientId}</span>
              </span>
              <Button type="button" size="xs" variant="outline" disabled={switching} onClick={() => void removeToken()}>
                Remove service token
              </Button>
            </div>
          )}
          <label htmlFor={`${id}-client-id`} className="text-xs text-muted-foreground">
            Client ID
          </label>
          <input
            id={`${id}-client-id`}
            className={`${inputClass} min-w-0 font-mono`}
            autoComplete="off"
            spellCheck={false}
            placeholder="0123abcd.access"
            value={clientId}
            onChange={(event) => setClientId(event.target.value)}
          />
          <label htmlFor={`${id}-client-secret`} className="text-xs text-muted-foreground">
            Client secret
          </label>
          <input
            id={`${id}-client-secret`}
            className={`${inputClass} min-w-0 font-mono`}
            type="password"
            autoComplete="off"
            value={clientSecret}
            onChange={(event) => setClientSecret(event.target.value)}
          />
        </fieldset>
        {/* Settings of the app itself: they must work while the server cannot be reached. */}
        <Button type="submit" size="sm" className="self-start" changesData={false} disabled={switching || !changed}>
          {sameServer ? "Save service token" : connectedServer === null ? "Connect" : "Switch"}
        </Button>
      </form>
      {connectedServer !== null && (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          disabled={switching}
          onClick={() => run(null)}
        >
          Use this computer's data
        </Button>
      )}
      {notice && (
        <p role="status" className="text-xs text-muted-foreground">
          {notice}
        </p>
      )}
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
    </section>
  );
}
