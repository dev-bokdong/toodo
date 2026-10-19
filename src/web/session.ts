/**
 * The web access token of `toodo-server`, kept in local storage. Every API request bears
 * it; a request the server rejects forgets it, so the app asks for it again.
 */

const STORAGE_KEY = "toodo-server-token";

const listeners = new Set<() => void>();

export function getToken(): string | null {
  return localStorage.getItem(STORAGE_KEY);
}

export function setToken(token: string): void {
  localStorage.setItem(STORAGE_KEY, token);
  listeners.forEach((listener) => listener());
}

export function forgetToken(): void {
  localStorage.removeItem(STORAGE_KEY);
  listeners.forEach((listener) => listener());
}

export function subscribeToken(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function authorization(token: string): Record<string, string> {
  return { Authorization: `Bearer ${token}` };
}
