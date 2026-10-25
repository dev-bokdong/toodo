# Voice in the Lead Chat

The web app's lead chat (Dori) takes spoken messages and can read the lead's replies aloud.
Both are done by the browser:

- **Voice input**: the microphone button next to Send starts the browser's speech
  recognition (Web Speech API) in the chosen speech language. The words show in the message
  box as they are recognized; after two seconds of quiet, or on the stop button, the message
  is sent. Escape drops it and keeps what was typed before. Browsers without speech
  recognition (Firefox) show no button; on a phone the keyboard's own dictation still works
  there.
- **Reading aloud**: off by default. The speaker button in the chat header, or
  Settings > General > "Read Dori's replies aloud", turns it on for this browser. New replies
  (not the history) are then read with the browser's speech synthesis, without Markdown
  markup, code blocks, or links, in pieces short enough to dodge Chrome's cut-off of long
  utterances. "Stop reading aloud" shows in the chat header, and over the Dori button while
  the chat is closed.
- Settings > General > "Speech language" (default: the browser's language) applies to both.
  It is kept per browser in local storage, as is the reading-aloud switch.

The server takes no part: no audio reaches `toodo-server`, and it needs no model or key.

The macOS app is the exception for voice input: its WebView has no working speech
recognition, so the app recognizes speech itself with the Mac's `SFSpeechRecognizer`, still
on the device and with the same quiet rule, buttons, and speech language. See
[docs/macos.md](macos.md#voice). The macOS app also takes a message spoken after the wake
word "Dori-ya" without a click, recognized by a model running in the app (see
[docs/wake-word.md](wake-word.md)). Dictated and wake word messages go out through the same
queue: one said while another is still sending waits its turn, and a failed one stays in
the chat with Retry, under the client ID it was heard with.

## How the Options Compared

Measured on the home server that runs `toodo-server` (Intel N150, 4 cores, no GPU) in
October 2026.

### Samples

Eight Korean chat messages of the kind sent to the lead ("show me the tasks in progress
 today", "once the pull request is merged, start the next task right away", ...), each read by two
synthetic neural voices (one female, one male): 16 clips of 3.5 to 4.8 seconds, 4.2 on
average. Synthetic speech is cleaner than a person at a desk or on a phone, so the error
rates below are a lower bound; they rank the options rather than predict real use.

Character error rate (CER) is the edit distance between the reference and the transcript
with spaces and punctuation removed, divided by the reference length, so a word spelled
with or without a space counts as equal.

### Speech to Text

| Option | CER | Time to text | Load on the server | Works in |
| --- | --- | --- | --- | --- |
| Web Speech API, ko-KR (Chrome 155) | 1.0% | final text at the end of speech; first words about 0.9 s after speech starts | none | Chrome (desktop, Android), Samsung Internet, Safari (with Siri on); not Firefox |
| faster-whisper `base`, int8 | 3.5% | 1.3 s per clip after upload | 3.2 cores busy, 350 MB | any browser that can record audio |
| faster-whisper `small`, int8 | 1.1% | 3.7 s per clip (up to 4.9 s) | 3 cores busy, 1.0 GB | same |
| faster-whisper `medium`, int8 | 0.0% | 10.1 s per clip | 3.1 cores busy, 2.8 GB | same |
| Hosted STT API | not measured | | | |

- **Web Speech.** Measured in a real Chrome with each clip fed as the recognition's audio
  track. In single-utterance mode the final text arrived before the clip's trailing silence
  ended; with continuous listening and the two-second quiet rule the app uses, the message
  was ready 2.4 s after the clip ended (the final text itself 0.3 s after). The errors were
  the loanword "meoji" (merge) heard as "mwoji" (what), and one past-tense ending. Chrome sends the audio to Google's speech
  service; Chrome 155 also reports Korean as downloadable for on-device recognition
  (`processLocally`), which a later change could prefer. Edge exposes the API but caniuse
  reports its events as not firing; Safari needs Siri enabled and does not offer it to web
  apps added to the home screen.
- **faster-whisper** (the CTranslate2 build of Whisper, `cpu_threads=4`, beam 5, language
  fixed to Korean) ran on the server itself. `small` matches Web Speech's accuracy but takes
  about as long as the speech itself and most of the CPU that the lead and its agents share;
  `medium` was exact but more than twice slower than real time. It would also need an
  audio upload endpoint, a model download, and a bundled inference library in
  `toodo-server`. It remains the fallback for browsers without speech recognition if one
  is needed.
- **Hosted APIs** (OpenAI, Google Cloud, Naver Clova and the like) need an account and an
  API key; none was available on the server, and none was bought for this comparison.

### Text to Speech

| Option | Intelligibility | Time to first sound | Load on the server | Works in |
| --- | --- | --- | --- | --- |
| Browser speech synthesis | depends on the device's voices | 0.3 to 0.5 s (Chrome's Google Korean voice) | none | every current browser |
| Supertonic 3 (int8, sherpa-onnx) on the server | 6% CER when transcribed back by Whisper `small`, all of it numbers written as digits | 3.3 to 5.8 s to synthesize a 22 to 60 character reply | 2.8 cores busy, 320 MB | any browser that can play audio |

- **Browser speech synthesis** uses the voices of the device: Chrome on desktop has the
  Google Korean network voice; Android uses the phone's engine (Google or Samsung TTS,
  on the device); Windows has "Microsoft Heami" once Korean speech is installed; Safari and
  iOS have "Yuna". Quality varies with the device, but it starts at once, works offline on
  phones, and stops instantly. Chrome stops an utterance after about fifteen seconds, which
  the app avoids by speaking a reply in pieces of up to 200 characters.
- **Supertonic 3** (Supertone's 99M-parameter on-device model, int8 through sherpa-onnx with
  two threads) sounds natural and was fully intelligible, but synthesizing a short reply
  took 0.7 times its spoken length on the N150, so the first sound comes seconds late unless
  replies are streamed sentence by sentence, and the server would need the model files, an
  ONNX runtime, and an audio endpoint.

### Choice

Web Speech recognition and browser speech synthesis: they were as accurate as the best local
model, the fastest by far, cost the server nothing, and need no new server dependency or
endpoint. Firefox has no voice input; a server-side Whisper `base` or `small` endpoint is
the way to add it should Firefox matter.
