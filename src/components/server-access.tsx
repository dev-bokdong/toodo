import { useState, useSyncExternalStore, type FormEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { inputClass } from "@/components/workspace-list";
import { authorization, getToken, setToken, subscribeToken } from "@/web/session";

/**
 * Shows the app once the browser holds an access token the server accepts; until then,
 * a form asks for it. The server keeps the token in `web-token` in its data directory.
 */
export function ServerAccess({ children }: { children: ReactNode }) {
  const token = useSyncExternalStore(subscribeToken, getToken);
  return token === null ? <TokenForm /> : children;
}

function TokenForm() {
  const [token, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const candidate = token.trim();
    setChecking(true);
    setError(null);
    try {
      const response = await fetch("/api/rpc/app_info", { method: "POST", headers: authorization(candidate) });
      if (response.ok) {
        setToken(candidate);
      } else if (response.status === 401) {
        setError("The server did not accept this token.");
      } else {
        setError(`The server answered ${response.status} ${response.statusText}.`);
      }
    } catch (failure) {
      setError(`Could not reach the server: ${String(failure)}`);
    } finally {
      setChecking(false);
    }
  }

  return (
    <main className="flex min-h-svh items-center justify-center p-6">
      <form onSubmit={submit} aria-label="Server access" className="flex w-full max-w-sm flex-col gap-3 rounded-lg border p-5">
        <h1 className="text-lg font-semibold">Toodo server</h1>
        <p className="text-sm text-muted-foreground">
          Enter the access token from <code className="text-xs">web-token</code> in the server's data directory.
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
        {error && (
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        )}
        <Button type="submit" disabled={checking || !token.trim()}>
          {checking ? "Checking..." : "Continue"}
        </Button>
      </form>
    </main>
  );
}
