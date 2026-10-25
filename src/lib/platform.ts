declare global {
  interface Window {
    /** The server the desktop app uses, set by the app before the page loads (see `src-tauri/src/connection.rs`). */
    __TOODO_SERVER__?: string | null;
  }
}

/** Whether this is the web build served by `toodo-server` rather than the desktop app. */
export const builtForWeb = import.meta.env.MODE === "web";

/**
 * The origin of the `toodo-server` the desktop app is connected to, or null when it shows
 * its own data (and in the web build, which is served by the server it talks to).
 */
export const connectedServer: string | null = builtForWeb ? null : (window.__TOODO_SERVER__ ?? null);

/**
 * Whether the data comes from `toodo-server`: the web build, or the desktop app connected
 * to a server. Its "Local" machine is the server, so local paths are POSIX paths there.
 */
export const runsOnServer = builtForWeb || connectedServer !== null;
