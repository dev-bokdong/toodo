import { useId, useReducer, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import type { AccessTokenInput, SavedServer } from "@/lib/bindings";
import { connectedServer, mobileApp } from "@/lib/platform";
import {
  serverName,
  signedIn,
  signOut,
  switchServer,
  useAccessToken,
  useConnection,
  useRemoveServer,
  useSaveServer,
  useSetAccessToken,
} from "@/lib/servers";
import { retryNow } from "@/web/connection";

/**
 * Where the desktop app's data lives: on this computer, or on one of the `toodo-server`s
 * saved in the app, which it connects to as one of the server's clients. The app keeps an
 * access token for each saved server, so switching between them needs no new sign-in. A
 * server behind Cloudflare Access also needs an Access service token, which the app keeps
 * per server and sends with every request to it; it never shows the secret again.
 */
export function ServerConnection() {
  const connection = useConnection();
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);

  async function use(target: string | null) {
    setSwitching(true);
    setError(null);
    setNotice(null);
    try {
      await switchServer(target);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
      setSwitching(false);
    }
  }

  const servers = connection.data?.servers ?? [];
  return (
    <section aria-label="Data" className="flex flex-col gap-3 text-sm">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium">Data</span>
        <span className="text-muted-foreground">
          {connectedServer !== null
            ? `On ${serverName(connection.data, connectedServer)}`
            : mobileApp
              ? "Not connected"
              : "On this computer"}
        </span>
      </div>
      {servers.length > 0 && (
        <ul aria-label="Saved servers" className="flex flex-col divide-y rounded-md border">
          {servers.map((saved) => (
            <SavedServerRow
              key={saved.url}
              saved={saved}
              busy={switching}
              onSwitch={() => void use(saved.url)}
              onError={setError}
              onNotice={setNotice}
            />
          ))}
        </ul>
      )}
      <AddServerForm onError={setError} />
      {connectedServer !== null && !mobileApp && (
        <Button
          variant="outline"
          size="sm"
          className="self-start"
          disabled={switching}
          onClick={() => void use(null)}
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

function SavedServerRow({
  saved,
  busy,
  onSwitch,
  onError,
  onNotice,
}: {
  saved: SavedServer;
  busy: boolean;
  onSwitch: () => void;
  onError: (message: string | null) => void;
  onNotice: (message: string | null) => void;
}) {
  const save = useSaveServer();
  const remove = useRemoveServer();
  const accessToken = useAccessToken(saved.url);
  const setAccessToken = useSetAccessToken();
  const [editingToken, setEditingToken] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(saved.name);
  const [confirmingRemove, setConfirmingRemove] = useState(false);
  // Signing out of another server changes only local storage; show the new state.
  const [, refresh] = useReducer((count: number) => count + 1, 0);
  const inUse = saved.url === connectedServer;
  const hasToken = signedIn(saved.url);

  function rename(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onError(null);
    save.mutate(
      { url: saved.url, name: name.trim() },
      { onSuccess: () => setRenaming(false), onError: (failure) => onError(failure.message) },
    );
  }

  function keepToken(token: AccessTokenInput | null) {
    onError(null);
    onNotice(null);
    setAccessToken.mutate(
      { url: saved.url, token },
      {
        onSuccess: () => {
          setEditingToken(false);
          if (inUse) retryNow();
          onNotice(
            token === null
              ? `Removed the service token of ${saved.name}.`
              : `Saved. The app sends the new service token to ${saved.name} from now on.`,
          );
        },
        onError: (failure) => onError(failure.message),
      },
    );
  }

  if (renaming) {
    return (
      <li className="p-2">
        <form onSubmit={rename} aria-label={`Rename ${saved.name}`} className="flex flex-wrap gap-2">
          <input
            aria-label={`Name of ${saved.url}`}
            className={`${inputClass} min-w-0 flex-1`}
            maxLength={80}
            value={name}
            onChange={(event) => setName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                setName(saved.name);
                setRenaming(false);
              }
            }}
            autoFocus
            required
          />
          <Button type="submit" size="sm" changesData={false} disabled={save.isPending || !name.trim()}>
            Save
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setName(saved.name);
              setRenaming(false);
            }}
          >
            Cancel
          </Button>
        </form>
      </li>
    );
  }

  return (
    <li className="flex flex-col gap-1.5 p-2" data-in-use={inUse || undefined}>
      <div className="flex flex-wrap items-baseline gap-x-2">
        <span className="font-medium break-words-keep">{saved.name}</span>
        {inUse && <span className="rounded-full border px-1.5 text-xs">In use</span>}
        <span className="ml-auto text-xs text-muted-foreground">{hasToken ? "Signed in" : "Not signed in"}</span>
      </div>
      <span className="text-xs break-all text-muted-foreground">{saved.url}</span>
      {accessToken.data && (
        <span className="text-xs text-muted-foreground">
          Access service token: <span className="font-mono break-all">{accessToken.data.clientId}</span>
        </span>
      )}
      {editingToken ? (
        <AccessTokenForm
          label={`Service token of ${saved.name}`}
          busy={setAccessToken.isPending}
          onSave={keepToken}
          onCancel={() => setEditingToken(false)}
        />
      ) : confirmingRemove ? (
        <div role="group" aria-label={`Remove ${saved.name}?`} className="flex flex-wrap items-center gap-2">
          <span className="text-xs">Remove {saved.name} and forget this app's tokens for it?</span>
          <Button
            variant="destructive"
            size="xs"
            disabled={remove.isPending}
            onClick={() =>
              remove.mutate(saved.url, {
                onError: (failure) => {
                  setConfirmingRemove(false);
                  onError(failure.message);
                },
              })
            }
          >
            Remove
          </Button>
          <Button variant="ghost" size="xs" autoFocus onClick={() => setConfirmingRemove(false)}>
            Keep
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap gap-2">
          {!inUse && (
            <Button size="xs" disabled={busy} aria-label={`Switch to ${saved.name}`} onClick={onSwitch}>
              Switch
            </Button>
          )}
          <Button variant="outline" size="xs" aria-label={`Rename ${saved.name}`} onClick={() => setRenaming(true)}>
            Rename
          </Button>
          <Button
            variant="outline"
            size="xs"
            aria-label={`${accessToken.data ? "Change" : "Add"} service token of ${saved.name}`}
            title="A Cloudflare Access service token, for a server behind Cloudflare Access"
            onClick={() => setEditingToken(true)}
          >
            {accessToken.data ? "Change service token" : "Add service token"}
          </Button>
          {accessToken.data && (
            <Button
              variant="outline"
              size="xs"
              aria-label={`Remove service token of ${saved.name}`}
              disabled={setAccessToken.isPending}
              onClick={() => keepToken(null)}
            >
              Remove service token
            </Button>
          )}
          {hasToken && (
            <Button
              variant="outline"
              size="xs"
              aria-label={`Sign in to ${saved.name} again`}
              title="Forget this app's token, to sign in again with a new one"
              onClick={() => {
                signOut(saved.url);
                refresh();
              }}
            >
              Sign in again
            </Button>
          )}
          {!inUse && (
            <Button variant="outline" size="xs" aria-label={`Remove ${saved.name}`} onClick={() => setConfirmingRemove(true)}>
              Remove
            </Button>
          )}
        </div>
      )}
    </li>
  );
}

/** The client ID and secret of a Cloudflare Access service token, both or neither. */
function AccessTokenFields({
  clientId,
  clientSecret,
  onClientId,
  onClientSecret,
}: {
  clientId: string;
  clientSecret: string;
  onClientId: (value: string) => void;
  onClientSecret: (value: string) => void;
}) {
  const id = useId();
  return (
    <>
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
        onChange={(event) => onClientId(event.target.value)}
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
        onChange={(event) => onClientSecret(event.target.value)}
      />
    </>
  );
}

function AccessTokenForm({
  label,
  busy,
  onSave,
  onCancel,
}: {
  label: string;
  busy: boolean;
  onSave: (token: AccessTokenInput) => void;
  onCancel: () => void;
}) {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSave({ clientId, clientSecret });
  }

  return (
    <form
      onSubmit={submit}
      aria-label={label}
      className="flex flex-col gap-1"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          onCancel();
        }
      }}
    >
      <AccessTokenFields
        clientId={clientId}
        clientSecret={clientSecret}
        onClientId={setClientId}
        onClientSecret={setClientSecret}
      />
      <div className="flex flex-wrap gap-2">
        {/* Settings of the app itself: they must work while the server cannot be reached. */}
        <Button
          type="submit"
          size="xs"
          changesData={false}
          disabled={busy || !clientId.trim() || !clientSecret.trim()}
        >
          Save service token
        </Button>
        <Button variant="ghost" size="xs" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function hostOf(address: string): string {
  try {
    return new URL(address).host || address;
  } catch {
    return address;
  }
}

function AddServerForm({ onError }: { onError: (message: string | null) => void }) {
  const save = useSaveServer();
  const setAccessToken = useSetAccessToken();
  const [address, setAddress] = useState("");
  const [name, setName] = useState("");
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const id = useId();
  const tokenEntered = clientId.trim() !== "" || clientSecret.trim() !== "";
  const busy = save.isPending || setAccessToken.isPending;

  async function add(url: string) {
    // The token first: a malformed one leaves no server saved without it.
    if (tokenEntered) await setAccessToken.mutateAsync({ url, token: { clientId, clientSecret } });
    await save.mutateAsync({ url, name: name.trim() || hostOf(url) });
  }

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onError(null);
    add(address.trim()).then(
      () => {
        setAddress("");
        setName("");
        setClientId("");
        setClientSecret("");
      },
      (failure: unknown) => onError(failure instanceof Error ? failure.message : String(failure)),
    );
  }

  return (
    <form onSubmit={submit} aria-label="Add a Toodo server" className="flex flex-col gap-1">
      <label htmlFor={`${id}-address`} className="text-xs text-muted-foreground">
        Toodo server address
      </label>
      <input
        id={`${id}-address`}
        className={inputClass}
        type="url"
        inputMode="url"
        placeholder="https://toodo.example"
        value={address}
        onChange={(event) => setAddress(event.target.value)}
        aria-describedby={`${id}-help`}
        required
      />
      <label htmlFor={`${id}-name`} className="text-xs text-muted-foreground">
        Server name
      </label>
      <div className="flex flex-wrap gap-2">
        <input
          id={`${id}-name`}
          className={`${inputClass} min-w-0 flex-1`}
          maxLength={80}
          placeholder={address.trim() ? hostOf(address.trim()) : "Home"}
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <Button type="submit" size="sm" changesData={false} disabled={busy || !address.trim()}>
          Add server
        </Button>
      </div>
      <details className="rounded-md border p-2 text-xs">
        <summary className="cursor-pointer">Cloudflare Access service token</summary>
        <div className="mt-1 flex flex-col gap-1">
          <p className="text-muted-foreground">
            Only for a server behind Cloudflare Access. The app sends the token with every request to that server;
            the secret is kept in the app and never shown again.
          </p>
          <AccessTokenFields
            clientId={clientId}
            clientSecret={clientSecret}
            onClientId={setClientId}
            onClientSecret={setClientSecret}
          />
        </div>
      </details>
      <p id={`${id}-help`} className="text-xs text-muted-foreground">
        {mobileApp
          ? "Switching to a server shows its data. The app signs in to each server once and keeps a token for each."
          : "Switching to a server shows its data instead of this computer's, which stays as it is. The app signs in to each server once and keeps a token for each."}
      </p>
    </form>
  );
}

/** The Android app's first screen, until it is connected to a server. */
export function ServerSetup() {
  return (
    <main className="mx-auto flex min-h-dvh max-w-md flex-col justify-center gap-4 p-6 pt-[calc(1.5rem+env(safe-area-inset-top))]">
      <h1 className="text-xl font-semibold tracking-tight">Connect to your Toodo server</h1>
      <p className="text-sm text-muted-foreground">
        Toodo on this phone shows the tasks and the lead chat of a Toodo server. Add the server, then switch to it.
      </p>
      <ServerConnection />
    </main>
  );
}
