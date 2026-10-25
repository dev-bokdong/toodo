/** Whether this is the web build served by `toodo-server` rather than the desktop app. */
export const builtForWeb = import.meta.env.MODE === "web";

/**
 * Whether the data comes from `toodo-server`, as in the web build. Its "Local" machine is the
 * server, so local paths are POSIX paths there.
 */
export const runsOnServer = builtForWeb;

/** Whether the app checks for updates: only the desktop app is updated by Toodo itself. */
export const checksForUpdates = !builtForWeb;
