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

### Wake Word

Say "Dori-ya" (do-ri-ya, in Korean) and then a message, in one breath or after a short pause,
and the message goes to the lead chat: with the window open, or closed to the menu bar.

- **Turning it on.** It is off until **Settings > General > Send a message by saying
  "Dori-ya"** is checked, which the app offers once it is connected to a server. The first
  time, it downloads its speech model (about 140 MB, from the model's upstream repository,
  checked against fixed checksums) into `wake-word-model` in the app data directory, and
  asks for the microphone if it was never asked before. Speech recognition permission is
  not needed: the wake word and the message are recognized on the Mac, by the model
  measured in [docs/wake-word.md](wake-word.md).
- **Only while the app runs.** It listens while Toodo runs, window shown or hidden, and
  starts again by itself when the app starts with the setting on. **Quit** closes the
  microphone. There is no login item and no system-wide hotword.
- **While it hears you.** The Mac plays the "Tink" sound, the menu bar shows
  "Listening", and the window shows what it hears at the top. Two seconds of quiet sends
  the message; nothing said within five seconds of the wake word, Esc, or the cancel button
  on the indicator sends nothing. The wake word alone sends nothing either.
- **Sending.** The message is sent like a typed one, to the server the app uses, also while
  the lead restarts or an outside lead is away (it waits on the server until a lead takes
  it). If the send fails, the message stays in the chat with **Retry**; it is retried by
  itself a few times, always under the same client ID, so it is never posted twice.
- **Its own voice.** While the app reads a reply aloud, and for 0.3 s after, the wake word
  is not listened for, so Dori cannot wake itself. It also holds while a message is
  dictated with the microphone button.
- **When the microphone goes away.** After the Mac sleeps or the input device changes, the
  app opens the default microphone again once audio stops coming. If the microphone is
  turned off for Toodo in **Privacy & Security**, Settings shows it with **Open System
  Settings**, and listening resumes by itself once it is allowed again.

The status line under the switch tells what the wake word is doing: downloading,
listening, paused while a reply is read aloud, or what went wrong.

## How It Is Built

- `src-tauri/tauri.macos.conf.json` (merged into `tauri.conf.json` on macOS): `.app` and
  `.dmg` bundles, `signingIdentity` `-` (ad-hoc), and `Entitlements.plist`, which allows
  audio input under the hardened runtime.
- `src-tauri/Info.plist` (merged into the app's): why the app asks for the microphone and
  speech recognition.
- `src-tauri/src/dictation/`: the dictation commands (`start_dictation`,
  `stop_dictation`, `cancel_dictation`, `open_privacy_settings`) and the
  `dictation-changed` event; `capture.rs`, the one owner of the microphone, which
  dictation and the wake word share; and `wake/`, the wake word (`get_wake_word`,
  `set_wake_word`, `cancel_wake_word`, `set_wake_word_speaking`, the events
  `wake-word-changed` and `wake-word-heard`). They are commands and events of the app, not
  of the server, so a connected app keeps them local (`src/client/core.ts`,
  `src/client/event.ts`); on Windows they reject.
- The wake word runs the sherpa-onnx recognizer, linked statically into the app from the
  `sherpa-onnx` crate's prebuilt libraries (downloaded when the app is built). The
  frontend sends what it hears through the voice outbox (`src/lib/voice-outbox.ts`), which
  also sends dictated messages.
- The app tells the page its system (`window.__TOODO_PLATFORM__`), which picks the native
  recognizer and leaves out the update checks on macOS (`src/lib/platform.ts`).
