# The macOS App

The macOS app is the desktop app built for Apple Silicon Macs and used as a client of a
`toodo-server`: it shows the server's tasks and lead chat like the Windows app connected to
a server, lives in the menu bar, and takes spoken messages for the lead with the Mac's own
speech recognition. It is meant for personal use: the build is ad-hoc signed, with no
Developer ID and no notarization.

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

Ad-hoc signing gives every build a different signature. macOS keys the microphone and
speech recognition permissions to it, so after installing a new build, it asks for them
again.

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
- **Voice Message** (once connected to a server) opens the lead chat and starts dictating
  a message, which is sent when you stop speaking.
- **Quit** (or Cmd+Q) exits the app.

## Voice

The macOS WebView (WKWebView) gives a page no working speech recognition, so the macOS app
dictates natively: `AVAudioEngine` takes the microphone and `SFSpeechRecognizer`
recognizes it in the speech language from **Settings > General** (Korean is `ko-KR`).
Everything else works as described in [docs/voice.md](voice.md): the words show in the
message box while you speak, the message is sent after two seconds of quiet or on the stop
button, and Esc drops it.

- **Permissions.** The first dictation asks for speech recognition, then for the
  microphone. If either was denied, the chat shows which and an **Open System Settings**
  button that opens the right pane of **Privacy & Security**; turn Toodo on there and
  press the microphone button again.
- **Network.** Apple's recognizer may send the audio to Apple; it needs a network
  connection unless the Mac has on-device recognition for the language. If it reports that
  recognition is not available, check that Dictation is on in
  **System Settings > Keyboard**.
- **Reading replies aloud** uses the WebView's speech synthesis with the Mac's voices
  (Korean: "Yuna"), unchanged from the browser.

### For a Wake Word

The audio runs in the app, not in the page, so it keeps running while the window is hidden
in the menu bar. A native wake-word detector can live next to it in
`src-tauri/src/dictation/` and, on hearing the wake word, call
`desktop::request_voice_message`: that shows the window and sends the
`voice-message-requested` event, upon which the lead chat dictates and sends a message,
exactly as the **Voice Message** menu item does.

## How It Is Built

- `src-tauri/tauri.macos.conf.json` (merged into `tauri.conf.json` on macOS): `.app` and
  `.dmg` bundles, `signingIdentity` `-` (ad-hoc), and `Entitlements.plist`, which allows
  audio input under the hardened runtime.
- `src-tauri/Info.plist` (merged into the app's): why the app asks for the microphone and
  speech recognition.
- `src-tauri/src/dictation/`: the dictation commands (`start_dictation`,
  `stop_dictation`, `cancel_dictation`, `open_privacy_settings`) and the
  `dictation-changed` event. They are commands of the app, not of the server, so a
  connected app keeps them local (`src/client/core.ts`); on Windows they reject.
- The app tells the page its system (`window.__TOODO_PLATFORM__`), which picks the native
  recognizer and leaves out the update checks on macOS (`src/lib/platform.ts`).
