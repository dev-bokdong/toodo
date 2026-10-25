# The macOS App

The macOS app is the desktop app built for Apple Silicon Macs and used as a client of a
`toodo-server`: it shows the server's tasks like the Windows app connected to a server,
and lives in the menu bar. It's meant for personal use: the build is ad-hoc signed, with
no Developer ID and no notarization.

## Getting the App

The [macOS workflow](../.github/workflows/macos.yml) builds the app on a GitHub-hosted
Apple Silicon runner for every pull request and push to `dev` or `main` that touches the
app, and on demand (`gh workflow run macos.yml --ref <branch>`). Each run keeps the
artifact `toodo-macos-arm64` with:

- `Toodo_<version>_aarch64.dmg`: the disk image to install from;
- `Toodo-macos-arm64.app.zip`: the same app, zipped with `ditto`.

Download it from the run's page, or with
`gh run download <run id> --name toodo-macos-arm64`.

Releases and the updater are Windows-only: the release workflow publishes no macOS build,
and the app has no "Check for Updates". To update, install a newer artifact over the old
app.

## Installing

1. Open the `.dmg` and drag **Toodo** to **Applications**.
2. The first time, macOS refuses to open an app that is not notarized. Open
   **System Settings > Privacy & Security**, and under "Toodo was blocked" choose
   **Open Anyway**, then **Open**. (Instead, remove the quarantine flag the download
   carries: `xattr -dr com.apple.quarantine /Applications/Toodo.app`.)

## Connecting to a Server

The app starts on its own empty data, as the Windows app does. Under **General** in
Settings, add the server's address with a name and choose **Switch**; the app restarts,
asks for `web-token` and a device name once, and from then on is a client of that server
(`connection.json` in `~/Library/Application Support/dev.toodo.app`). Everything else,
including several saved servers and a server behind Cloudflare Access, works as described in
[The Desktop App as a Client](server.md#the-desktop-app-as-a-client). While the server
cannot be reached, the app shows why and keeps trying; it picks up again when the server
comes back.

The app's own data mode (local workspaces driving herdr) is built for Windows and is not a
goal on the Mac.

## The Menu Bar

The Toodo icon in the menu bar keeps the app running while its window is closed:

- Closing the window hides it; clicking the menu bar icon, **Show Toodo** in its menu, or
  the Dock icon brings it back.
- **Quit** (or Cmd+Q) exits the app.

## How It Is Built

- `src-tauri/tauri.macos.conf.json` (merged into `tauri.conf.json` on macOS): `.app` and
  `.dmg` bundles and `signingIdentity` `-` (ad-hoc).
- The app tells the page its system (`window.__TOODO_PLATFORM__`), which leaves out the
  update checks on macOS (`src/lib/platform.ts`).
