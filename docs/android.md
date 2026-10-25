# Android App

Toodo runs on an Android phone as a client of a `toodo-server` (see [server.md](server.md)):
the same app as on the desktop, built for Android by Tauri 2, connected to a server. It has
no data of its own, no tray, and no self-update; it is installed from an APK file, outside
any app store.

## Installing

1. Download the `toodo-android` artifact of the latest `Android` workflow run on `dev`
   (Actions > Android), and unzip it: it holds `toodo-X.Y.Z-android-arm64.apk`.
2. Copy the APK to the phone and open it. Android asks once to allow installing apps from
   the app that opens it (Files, a browser, ...).
3. A newer APK installs over the installed one and keeps its data (the server address, the
   sign-in, the Cloudflare Access service token), since every APK from CI is signed with
   the same key (see [Signing](#signing)).

The APK is built for 64-bit ARM phones (`arm64-v8a`), which covers current Samsung and
other Android phones; Android 7.0 (API 24) is the oldest version it installs on.

## Connecting to a Server

On its first start the app asks for the server's address, as the desktop app does under
Settings > General > Data. A server behind Cloudflare Access also needs an Access service
token there. The app then reloads, shows the server's sign-in, and from then on opens
straight into the server's data. Settings > General > Data switches to another server.

The app sends every request to the server from its Rust side (the `remote` plugin in
`src-tauri/src/remote/`), as the desktop app does. On Android the server's certificate is
checked against the Mozilla root certificates bundled into the app (`webpki-roots`), not
the phone's certificate store, so a server must use a certificate from a public authority
(Cloudflare's are), or plain `http` on a trusted network.

## Talking to Dori

- **Launcher shortcut**: long-press the Toodo icon and choose "Talk to Dori" (drag it out
  to put it on the home screen). It opens the lead chat listening for a spoken message.
- **Links**: `toodo://chat` opens the lead chat, `toodo://chat/voice` opens it listening.
  They work whether the app is running or not (Tauri's deep-link plugin: `getCurrent` at
  start, `onOpenUrl` while running). A link names no server and carries no token: the app
  uses the server it is connected to. Voice input starts once the app has reached the
  server, is signed in, and has the microphone; before that, the request waits.
- **Voice input**: the microphone button next to Send, as in the web app
  ([voice.md](voice.md)). The words show in the message box as they are recognized, and the
  message is sent when the speaker stops. Leaving the app while listening drops the message
  and frees the microphone.

### Speech Recognition

The Android WebView has no Web Speech API speech recognition (Chrome and Samsung Internet
do, the WebView does not), so the app dictates with the phone's own: the app's speech
plugin (`src-tauri/gen/android/app/src/main/java/dev/toodo/app/SpeechPlugin.kt`) runs
Android's `SpeechRecognizer`, and `src/lib/native-speech.ts` gives it the shape of the
browser's recognition, so the chat drives both alike. Should a WebView offer the Web Speech
API, the app tries it first and hands over to the phone's recognition when it fails before
hearing anything (`withNativeFallback` in the same file).

- The language is Settings > General > Speech language (Korean by default on a Korean
  phone).
- `SpeechRecognizer` uses the phone's speech service: Google's on most phones, Samsung's
  on Samsung phones set up that way. It listens for one utterance and stops after about
  two seconds of quiet.
- The microphone permission is asked for the first time voice input starts. Denied once,
  the next tap asks again; denied for good, the chat says so and offers "Open app
  settings", where the microphone is allowed again.

### Foreground Wake Word

Settings > General > Voice in the lead chat > **Listen for Dori (do-ri-ya)** enables
hands-free Korean messages. It is off by default and asks for microphone permission when
enabled. Say the wake word followed by a sentence, in one breath if preferred. The chat
opens with a haptic cue when the wake word is detected; **Cancel voice message** (or Escape)
discards the current utterance. Android uses haptic feedback rather than the audible cue
described in part A: a speaker tone could enter the live recognizer or mask the sentence's
first syllable. The phone's system haptic setting applies. Two seconds without another word finishes the message; a wake word
without a sentence times out without sending.

The detector and sentence recognizer share one local sherpa-onnx stream, using the Korean
zipformer and pronunciation rule selected in [wake-word.md](wake-word.md). There is no
handoff to `SpeechRecognizer` and no microphone audio is uploaded. The app downloads and
verifies the Korean model from its upstream release on first use; the APK contains the
runtime, not the model. The manual microphone button still uses the phone's speech service.

Listening is limited to the visible, resumed, unlocked Activity. Leaving the app, locking
the screen, revoking permission, a call/audio conflict, manual dictation, or reading a reply
aloud pauses capture. Returning rechecks the setting and permission without opening another
permission prompt. There is no microphone service, background launch, or battery exemption.

A completed voice message and its client id are saved on the phone before sending. If the
connection fails, the chat keeps the transcript and offers **Retry voice message**; reconnecting
or reopening also retries. The same id is used until the server accepts the message, so a
lost response does not create another chat message. Switching servers never sends the saved
message to the new server: return to the original server to finish delivery. A pending
message pauses new wake-word capture rather than overwriting unsent words.

#### Device Acceptance

Automated bridge and JVM tests do not establish microphone accuracy, latency, power use, or
Samsung lifecycle behavior. Before accepting a build on a real phone, record its APK commit,
phone model, Android/One UI version, microphone route, and the following evidence:

- With the setting off or microphone denied, no recording indicator and no message.
- At least 30 fresh real-voice wake utterances, including run-on sentences near the phone
  and across the room. Miss rate at most 10%, p95 wake-end-to-detection at most one second,
  zero clipped first syllables and zero duplicated messages; the stored text excludes the
  wake word. Run one hour of ordinary conversation/video negatives, with at most one false
  accept. Use the part A [evaluation protocol](wake-word.md#limits-and-next-steps).
- Home, another Activity, screen off/lock, rapid pause/resume, permission revocation, a phone
  call, manual dictation, and TTS each stop or suspend recording; returning starts only one
  detector. Check cancellation and a wake word followed by silence.
- Disconnect after speaking; reconnect and restart the app. The transcript and client id
  survive, and the server stores exactly one message. Repeat after switching servers.
- Against `TOODO_LEAD=external`, verify the message in the server's chat and the external
  consumer's inbox/feed, without a built-in Lead pane.

#### Matched Power Comparison

Run two 30-minute foreground trials on the same Samsung phone, wake word **off** and **on**.
Download the model before either trial. Keep brightness (disable automatic brightness),
screen timeout, Wi-Fi/network traffic, app view, ambient temperature, audio stimulus, and
starting charge/temperature matched. Use wireless ADB with the USB charger disconnected;
simulating an unplugged battery while charging does not measure discharge. Let the phone
cool between trials and repeat in reverse order when practical.

Capture these read-only snapshots before, during, and after each trial:

```sh
adb shell dumpsys cpuinfo
adb shell dumpsys meminfo dev.toodo.app
adb shell dumpsys battery
adb shell dumpsys thermalservice
adb shell dumpsys batterystats dev.toodo.app
```

Keep the CPU sampling interval reported by `cpuinfo` with the app's process row; report
mean and peak process CPU, not a single instantaneous reading. Report PSS/RSS memory,
battery level/charge-counter delta, battery temperature and thermal status, plus observed
surface heat. Temperature from `battery` is not a measurement of skin temperature.
Retain raw snapshots locally and put only the app's summarized measurements in the result.
See Android's `dumpsys` documentation for device-dependent output fields.

| Real-phone evidence | Status |
| --- | --- |
| Recognition gate and uninterrupted first syllable | Not yet measured |
| Foreground lifecycle and audio conflicts | Not yet verified on a phone |
| 30-minute off/on CPU and memory | Not yet measured |
| 30-minute off/on battery and heat | Not yet measured |

The N150 measurements in part A are not Android power measurements and must not be used to
fill this table.

Also measure the time from resuming the app, cancelling a capture, finishing TTS, and
acknowledging a message until **Listening for Dori** appears. This implementation verifies
the cached files and reloads the recognizer on each capture restart; its startup CPU and
latency are not represented by a steady-state idle measurement. Include the same number
of these transitions in both matched trials.

## Building

CI builds the APK in `.github/workflows/android.yml` on every pull request and push to
`dev` and `main` that touch the app, and keeps it as the run's `toodo-android` artifact.

To build it locally (Linux or macOS):

- JDK 17, the Android SDK (platform tools, build tools, a platform) and the NDK; set
  `JAVA_HOME`, `ANDROID_HOME`, and `NDK_HOME`.
- The Rust targets: `rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
npm ci
bash scripts/android-wake-prepare.sh           # pinned Android recognition runtime
npx tauri android build --apk --target aarch64   # a phone
npx tauri android build --apk --target x86_64    # the emulator
```

The APK is in `src-tauri/gen/android/app/build/outputs/apk/`. Without a signing key (see
below) a release build is unsigned and does not install; `--debug` builds one signed with
the local debug key instead. `npx tauri android dev` runs the app on a connected phone or
an emulator with the Vite dev server.

`src-tauri/gen/android/` is the Android project that Tauri generated; it is kept in the
repository with the app's own parts: the speech plugin, the launcher shortcut
(`res/xml/shortcuts.xml`), the microphone permission in `AndroidManifest.xml`, and the
signing setup in `app/build.gradle.kts`. Tauri regenerates the files ignored by its
`.gitignore` on every build.

## Signing

Android installs an update only when it is signed with the key of the installed app, so
every APK must come from one key, under the alias `toodo`. It is kept in two repository
secrets:

- `ANDROID_KEY_BASE64`: the keystore (PKCS12), base64-encoded.
- `ANDROID_KEY_PASSWORD`: the password of the keystore and of the key.

The workflow writes `src-tauri/gen/android/keystore.properties` from them (ignored by git),
and `app/build.gradle.kts` signs release builds with it; `apksigner verify --print-certs`
in the workflow log shows the certificate. Pull requests from forks get no secrets and
build a debug APK instead, which does not install over the released app.

For a local release build signed with the same key, write `keystore.properties` with
`keyAlias`, `password`, and `storeFile` (the keystore's path). If the key is lost, the
installed app cannot be updated: uninstall it (which drops its data) and install an APK
signed with the new key.
