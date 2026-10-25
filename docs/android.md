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
API, the app uses it first.

- The language is Settings > General > Speech language (Korean by default on a Korean
  phone).
- `SpeechRecognizer` uses the phone's speech service: Google's on most phones, Samsung's
  on Samsung phones set up that way. It listens for one utterance and stops after about
  two seconds of quiet.
- The microphone permission is asked for the first time voice input starts. Denied once,
  the next tap asks again; denied for good, the chat says so and offers "Open app
  settings", where the microphone is allowed again.

## Building

CI builds the APK in `.github/workflows/android.yml` on every pull request and push to
`dev` and `main` that touch the app, and keeps it as the run's `toodo-android` artifact.

To build it locally (Linux or macOS):

- JDK 17, the Android SDK (platform tools, build tools, a platform) and the NDK; set
  `JAVA_HOME`, `ANDROID_HOME`, and `NDK_HOME`.
- The Rust targets: `rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
npm ci
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
