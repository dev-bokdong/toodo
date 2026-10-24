import { useState, useSyncExternalStore, type FormEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import { guessDeviceLabel, signIn, TokenRejected } from "@/web/devices";
import { getToken, setToken, subscribeToken } from "@/web/session";

/**
 * Shows the app once the browser holds an access token the server accepts; until then,
 * a form asks for it. Signing in with the shared token from `web-token` in the server's
 * data directory registers this browser as a device with a token of its own.
 */
export function ServerAccess({ children }: { children: ReactNode }) {
  const token = useSyncExternalStore(subscribeToken, getToken);
  return token === null ? <TokenForm /> : children;
}

function TokenForm() {
  const [token, setDraft] = useState("");
  const [label, setLabel] = useState(() => guessDeviceLabel(navigator.userAgent));
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setChecking(true);
    setError(null);
    try {
      setToken(await signIn(token.trim(), label.trim()));
    } catch (failure) {
      if (failure instanceof TokenRejected) {
        setError(failure.message);
      } else if (failure instanceof TypeError) {
        setError(`Could not reach the server: ${String(failure)}`);
      } else {
        setError(failure instanceof Error ? failure.message : String(failure));
      }
    } finally {
      setChecking(false);
    }
  }

  return (
    <main className="flex min-h-svh items-center justify-center p-6">
      <form onSubmit={submit} aria-label="Server access" className="flex w-full max-w-sm flex-col gap-3 rounded-lg border p-5">
        <h1 className="text-lg font-semibold">Toodo server</h1>
        <p className="text-sm text-muted-foreground">
          Enter the access token from <code className="text-xs">web-token</code> in the server's data directory. This
          browser then gets a token of its own, which you can revoke under Devices.
        </p>
        <input
          className={inputClass}
          type="password"
          autoComplete="current-password"
          aria-label="Access token"
          placeholder="Access token"
          value={token}
          onChange={(event) => setDraft(event.target.value)}
          required
        />
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Device name
          <input
            className={inputClass}
            aria-label="Device name"
            maxLength={80}
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            required
          />
        </label>
        {error && (
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        )}
        <Button type="submit" disabled={checking || !token.trim() || !label.trim()}>
          {checking ? "Checking..." : "Continue"}
        </Button>
      </form>
    </main>
  );
}
