declare global {
  interface Window {
    /** The server the desktop app uses, set by the app before the page loads (see `src-tauri/src/connection.rs`). */
    __TOODO_SERVER__?: string | null;
    /** Set by the Android app before the page loads. */
    __TOODO_MOBILE__?: boolean;
    /** The system the desktop app runs on (`windows`, `macos`, `linux`), set by the app before the page loads. */
    __TOODO_PLATFORM__?: string;
  }
}

/** Whether this is the web build served by `toodo-server` rather than the desktop app. */
export const builtForWeb = import.meta.env.MODE === "web";

/** Whether this is the Android app, which is always a client of a `toodo-server`. */
export const mobileApp = !builtForWeb && window.__TOODO_MOBILE__ === true;

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

/** The system the desktop app runs on, or null in the web build. */
export const appPlatform: string | null = builtForWeb ? null : (window.__TOODO_PLATFORM__ ?? null);

/**
 * Whether dictation goes through the macOS app rather than the WebView: WKWebView has no
 * working speech recognition, so the app listens itself (`src-tauri/src/dictation/`).
 */
export const nativeDictation = appPlatform === "macos";

/**
 * Whether the app checks for updates: releases carry updater artifacts for Windows only; the
 * Android and macOS apps are updated by installing a newer build.
 */
export const checksForUpdates = !builtForWeb && !mobileApp && appPlatform !== "macos";
