/// <reference types="vitest/config" />
import path from "node:path";
import process from "node:process";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { pwa } from "./vite-pwa.ts";

const host = process.env.TAURI_DEV_HOST;

const src = path.resolve(import.meta.dirname, "./src");

/**
 * The web build (`--mode web`), served by `toodo-server`, swaps the Tauri APIs for the
 * modules in `src/web`, which talk to the server over HTTP.
 */
const webModules = {
  "@tauri-apps/api/core": "core",
  "@tauri-apps/api/event": "event",
  "@tauri-apps/plugin-dialog": "dialog",
  "@tauri-apps/plugin-notification": "notification",
  "@tauri-apps/plugin-opener": "opener",
  "@tauri-apps/plugin-process": "process",
  "@tauri-apps/plugin-updater": "updater",
};

/** Where `npm run dev:web` forwards API and MCP requests: a running `toodo-server`. */
const server = process.env.TOODO_SERVER ?? "http://127.0.0.1:47616";

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), ...(mode === "web" ? [pwa()] : [])],
  resolve: {
    alias: [
      ...(mode === "web"
        ? Object.entries(webModules).map(([module, file]) => ({
            find: new RegExp(`^${module}$`),
            replacement: path.join(src, "web", `${file}.ts`),
          }))
        : []),
      { find: "@", replacement: src },
    ],
  },

  // Keep Rust compiler errors visible in `tauri dev`.
  clearScreen: false,
  server: {
    // Tauri expects a fixed port; fail if it is not available.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
    // `ws` passes the pane view's WebSocket through; the Host header stays the dev server's,
    // which is what the server compares the Origin with.
    proxy: mode === "web" ? { "/api": { target: server, ws: true }, "/mcp": server } : undefined,
  },

  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
}));
