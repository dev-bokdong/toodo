/**
 * Whether this is the web build served by `toodo-server` rather than the desktop app.
 * Its "Local" machine is the server, so local paths are POSIX paths there.
 */
export const runsOnServer = import.meta.env.MODE === "web";
