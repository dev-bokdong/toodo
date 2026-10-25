# Wake Word: Engine and Capture Verification

Saying the wake word "도리야" (do-ri-ya) and then a sentence should send that sentence to the
lead chat without touching the computer. This document is part A of that work: it picks a
Korean wake word engine by measurement, proves that a sentence said in one breath right
after the wake word reaches the recognizer whole, and fixes the engine/audio contract the
platform integrations build on (the macOS app, then Windows and Android). The prototype in
`scripts/wake-word/` is a measuring harness; the macOS app's integration (part B) is
described in [The macOS App](#the-macos-app) below.

## Choice

- **Detector**: the sherpa-onnx streaming Korean zipformer transducer
  (`sherpa-onnx-streaming-zipformer-korean-2024-06-16`, int8 encoder and joiner), decoded with
  modified beam search biased toward the wake word ("hotword" score 2.0), and a wake word rule
  on its partial transcript: three syllables at the start of an utterance whose middle
  syllable is pronounced "ri" and which are within two jamo edits of do-ri-ya.
- **Sentence**: the same recognizer stream keeps decoding after the wake word; the message is
  the tokens after the wake word, ended by two seconds without a new word (or five seconds
  without any word after the wake word, which cancels). There is no hand-off, so nothing can
  be lost or repeated at the switch.
- **Hand-off to a platform recognizer** (Apple Speech, Android `SpeechRecognizer`), if a
  platform prefers its own: replay from a PCM ring buffer starting where the utterance
  began (at least 0.6 s before the wake word), then drop the wake word the second
  recognizer hears again at the same position.
- Porcupine is not used: it supports Korean, but Picovoice states it has no plan for personal
  or non-commercial use, and it needs an AccessKey with usage reporting. openWakeWord ships
  only English models under a non-commercial license.

Measured against the proposed gate, on synthetic positives and real negative speech:

| Gate | Target | Chosen detector |
| --- | --- | --- |
| Miss rate | at most 10% | 10.7% overall (16 of 150); 2.2% on the three Korean voices (1 of 45) |
| False accepts | at most 1 per hour | 0.76 per hour (1 in 78.4 min of read Korean speech) |
| Wake word end to detection, p95 | at most 1 s | 0.71 s (median 0.51 s) |
| Clipped or duplicated sentences | 0 | 0 lost to the hand-off, 0 duplicated (one accented clip misrecognized at its start) |
| CPU, RSS | record | 16.6% of one N150 core, 266 MB |

## Samples

No microphone or Mac was available on the machine that ran the measurements (an Intel N150
home server), so the positives are synthetic:

- **Positives**: 150 clips from ten neural voices (Microsoft Edge TTS): the three Korean
  voices and seven multilingual voices that speak Korean with a foreign accent, at speaking
  rates from -10% to +25%. Per voice: ten run-on clips "도리야 <sentence>" said as one text,
  one with no spaces at all ("도리야오늘할일알려줘"), three with the wake word and the sentence
  synthesized apart and joined by 0.4 to 1.0 s of silence (about 2 s of quiet in total, with
  the voices' own padding), and the wake word alone. Ten Toodo-style sentences ("오늘 할 일
  알려줘", "풀 리퀘스트가 머지되면 다음 작업을 바로 시작해", ...). The wake word's end time comes
  from the voice's word-boundary events.
- **Near misses**: 60 clips, 15 sentences by four voices, built to sound like the wake word:
  names that rhyme with it at the start of a sentence (토리야, 노리야, 도라야, 우리야), the
  word 도리 (duty) ending a sentence ("...자식의 도리야", which is the wake word's exact
  pronunciation), 돌이야 (also the same pronunciation), 도리어, 도리도리, 오리야, 소리야,
  고리야, 도리토스.
- **Negatives**: the test split of the Zeroth Korean corpus (CC BY 4.0): 457 sentences read
  by real speakers, 71 min of speech, joined with 0.3 to 1.5 s gaps into 78.4 min of audio.
  None of it is a wake word call.

Synthetic voices are cleaner than a person across the room, and read speech is calmer than
television or a meeting, so the numbers rank the options and size the costs; they do not
replace the real-voice test the integration still needs (see Limits).

## How the Options Compared

### Engines

| Engine | Korean wake word | Result here | License for this use |
| --- | --- | --- | --- |
| Porcupine (Picovoice) | supported, custom phrases trained in the Console | not run | needs a plan; the FAQ says there is no personal or non-commercial plan; AccessKey with usage metadata; the Rust SDK is no longer maintained (since July 2025) |
| openWakeWord | no: English-only pretrained models, custom training needs an English-centric TTS pipeline | not run | code Apache-2.0, pretrained models CC BY-NC-SA 4.0 |
| sherpa-onnx keyword spotter on the Korean zipformer | open vocabulary: the wake word as token sequences | 79/150 detected (47% miss), 0 false accepts in 78.4 min, p95 0.64 s | code Apache-2.0; model see below |
| sherpa-onnx streaming Korean ASR, greedy, wake word rule | from the transcript | see below | same |
| **sherpa-onnx streaming Korean ASR, hotword-biased, wake word rule** | from the transcript | **chosen** | same |
| SenseVoice (sherpa-onnx, multilingual incl. Korean) | from the transcript of the utterance head | garbled output even on its own Korean sample with sherpa-onnx 2.0.0; not usable as installed | own model license (not reviewed, since it did not work) |
| Whisper (faster-whisper) | from the transcript | not run as a detector: `base` already took 3 cores for 1.3 s per 4 s clip on this machine (see [voice.md](voice.md)) | MIT |

The keyword spotter is the obvious first try, since it needs no training: the wake word is
given as BPE token sequences, including the spelling 돌이야 that is pronounced the same.
Its recall peaked at boost 3.0 (39 of the first 60 positives) and fell at higher boosts,
and the installed version returns no token times, so it cannot place the wake word's end.

The recognizer itself rarely writes 도리야 (it is a rare word for the model): the greedy
transcript says 둘이야, 들이야, 돌이야, 오리야, or just 이야. Biasing the beam search toward
도리야 and matching by pronunciation instead of spelling fixed most of that. The rule
compares jamo after moving a final consonant onto a following silent initial (돌이야 becomes
도리야), so 둘이야 and 들이야 are one edit away, and 도시의 or 고기야 (heard in the
negatives) have no "ri" in the middle and never match.

### Detection

| Detector and rule | Detected | Miss | Latency p50 / p95 | False accepts in 78.4 min | Per hour |
| --- | --- | --- | --- | --- | --- |
| Keyword spotter, boost 3.0, threshold 0.05 | 79/150 | 47.3% | 0.51 / 0.64 s | 0 | 0.00 |
| ASR greedy, 2 edits + start | 119/150 | 20.7% | 0.51 / 0.71 s | 1 | 0.76 |
| ASR hotword, exact spelling 도리야 | 64/150 | 57.3% | 0.53 / 0.71 s | 0 | 0.00 |
| ASR hotword, 1 edit + start | 122/150 | 18.7% | 0.51 / 0.71 s | 0 | 0.00 |
| **ASR hotword, 2 edits + start (chosen)** | 134/150 | 10.7% | 0.51 / 0.71 s | 1 | 0.76 |
| ASR hotword, 3 edits + start | 146/150 | 2.7% | 0.53 / 0.7 s | 6 | 4.58 |

By kind of clip, for the chosen rule: run-on 85/100, no spaces 9/10, paused 30/30, wake word
alone 10/10. By voice: the three Korean voices missed 1 of 45 clips (2.2%); the seven
foreign-accented voices missed 15 of 105 (14%), mostly where the recognizer heard 요리야,
느리야, 두리아, or nothing before the sentence. The single false accept in the negatives was
the start of a sentence beginning "오히려" (rather), matched by a partial transcript before
the recognizer revised it; two others seen before the "ri" rule (도시의, 곡이야) are gone.

- Rule "start" means the previous word started at least 0.5 s earlier. It removes the real
  collision in Korean: "...지켜야 할 도리야" ("it is one's duty") ends a sentence with the
  exact sound of the wake word, and fired every time without the rule and never with it.
- What still fires are names at the start of an utterance that differ by one sound
  (토리야, 노리야, 도라야, 우리야, 도리어). Saying "Tori-ya, come here" to a dog would wake
  Dori; no engine can tell these apart without being trained on the owner's voice.
- Latency is the time from the end of the wake word (voice word timings) to the 100 ms audio
  chunk after which the rule matched, decoding included. A run-on sentence matched as soon as
  "ya" was decoded, before the sentence's first word.

### Cost

| Detector (one thread) | Input | CPU, 1 core = 100% | Peak RSS | Model load |
| --- | --- | --- | --- | --- |
| Hotword ASR (chosen) | speech | 16.6% | 266 MB | 3.1 CPU s |
| Hotword ASR (chosen) | near-silence | 16.5% | 268 MB | 2.9 CPU s |
| Greedy ASR | speech | 14.8% | 261 MB | 3.0 CPU s |

Measured with `cost.py` over 180 s of audio fed as fast as possible on the Intel N150 while
other jobs kept the machine at a load average of 15 to 20, so these are upper bounds there.
The streaming encoder runs on every chunk, so silence costs as much as speech; a voice
activity detector in front (Silero VAD, about 2 MB, in sherpa-onnx) could skip the encoder in
silence while the ring keeps the 0.6 s before speech starts, but was not measured here. The
model files are 141 MB (int8 encoder 127 MB, decoder 11 MB, int8 joiner 3 MB). Heat could not
be measured meaningfully on this server.

## Sentence Capture Without Gaps

The risk in "도리야오늘할일알려줘" said in one breath is the switch from the detector to the
sentence recognizer: the detector fires 0.3 to 0.5 s after the wake word ends, by which time
the first syllable of the sentence has already been spoken. Each strategy below was run over
every positive with a sentence; a message is "clipped" when the alignment with the reference
sentence starts with a deletion (the start of the sentence is missing) and "duplicated" when
it starts with an insertion (part of the wake word was sent along).

- **live** is what a naive integration does: start the sentence recognizer when the detector
  fires. It loses the start of most run-on sentences.
- **continue** is the choice: no switch at all.
- **replay** proves the ring buffer: a second recognizer is fed from the ring starting at the
  sample where the detector's current utterance began, then live chunks, and drops the wake
  word it hears again. With the same model and the same samples it must reproduce the
  detector's transcript, so any sample lost or read twice at the switch would show up as a
  different message.
- **replay-late** starts the second recognizer 0.6 s before the wake word, as a platform
  recognizer that is only handed the wake word and what follows would be. Three things
  mattered: a fresh stream started 0.3 s before the wake word dropped the first words of the
  sentence ("오늘"), 0.6 s did not (1.0 s was no better); the wake word must be dropped by its
  position in the new transcript, not by spelling (the second recognizer may hear 놀이하
  where the detector heard 놀이야) and not by a fixed time (token times of the two
  recognizers differ by up to 0.15 s, the length of a syllable); and the match must have "ri"
  as its middle syllable, or "이야 리뷰" (a wake word heard as just "이야") cuts the "리" of
  the sentence. A fresh stream can still decode the boundary differently from the detector.

| Strategy | Sentences detected | Clipped | Duplicated | Mean CER |
| --- | --- | --- | --- | --- |
| live (start at detection, no ring) | 124/140 | 94 | 0 | 49.0% |
| **continue (same stream, chosen)** | 124/140 | 1 | 0 | 8.1% |
| replay from the ring at the utterance start | 124/140 | 1 | 0 | 8.1% |
| replay from the ring 0.6 s before the wake word | 124/140 | 3 | 0 | 8.8% |

- `live` lost the start of all 85 detected run-on sentences and all 9 without spaces; only
  the paused clips survived, because there the detector fires before the sentence starts.
- The one clip flagged under `continue` and `replay` (`v4-runon-s3`, an accented voice) is a
  misrecognition, not lost audio: decoding the whole clip from its first sample gives the same
  "포 위스가" for "풀 리퀘스트가". The ring `replay` produced exactly the `continue` message for
  all 124 detected clips.
- `replay-late` adds two boundary errors on top of that clip: one where the fresh stream merged
  "야오" into one syllable ("놀이원을"), one where it stopped after two words. A platform
  recognizer fed only from the wake word on carries this kind of risk and must be measured.
- A wake word alone, or followed by five seconds of silence, ends with an empty message,
  which must not be sent.

## Engine and Audio Contract

For the macOS integration (#315) and the Windows (#310) and Android (#311) ports. The
sherpa-onnx runtime has C, Rust (`sherpa-onnx` crate), Kotlin/Java, and Swift bindings, and
the model is the same file on every platform.

Audio:

1. One capture owner per process opens the default input device only while the wake word
   setting is on and the app runs (window shown or hidden in the tray or menu bar), and
   closes it on quit or when the setting is turned off. No other component opens the
   microphone while it runs.
2. The owner delivers 16 kHz mono float32 samples in chunks of 100 ms (resampled from the
   device rate), each tagged with its absolute sample index since capture start.
3. Every chunk is written to a ring buffer of 30 s (1.9 MB) before it is decoded. A
   consumer reads any range `[start, end)` still in the ring by absolute index; no sample is copied twice
   or skipped at a hand-off because both sides use the same index.
4. Detection pauses while the app speaks (reading replies aloud) and for 0.3 s after, so
   Dori's own voice cannot wake it; the ring keeps filling.
5. A device change or a lost permission ends capture with an error state the UI shows; the
   owner reopens the default device when it returns, and the indices restart at zero.

Engine:

6. `Detector.feed(chunk) -> Wake | None`, where `Wake = { start, end, detected_at }` in
   seconds on the capture clock. Model: the streaming Korean zipformer int8, one thread,
   modified beam search with hotword 도리야 score 2.0, endpoint after 0.8 s of trailing
   silence; rule: three syllables, the middle one pronounced "ri", within 2 jamo edits of
   do-ri-ya, previous word 0.5 s or more earlier.
7. After a wake word the same stream keeps decoding. The sentence ends 2 s after the last
   new word, or 5 s after the wake word if no word followed (cancel). Escape, or a click on
   the listening indicator, also cancels.
8. The result is `{ message, wake, ended_by: "quiet" | "timeout" | "cancel" }`. An empty
   message, a timeout, or a cancel sends nothing. A non-empty message goes to the lead chat
   with a client ID made once per wake event, so a retry after a failed send cannot post it
   twice; the transcript stays visible until the send succeeds.
9. A platform recognizer may replace the sentence step only through replay: feed it
   `ring[utterance start, now)` (or at least `ring[wake.start - 0.6 s, now)`) and then live
   chunks, and drop the tokens up to the wake word it hears again (pronunciation within 3 edits, last syllable within 0.3 s of
   `wake.end`), falling back to tokens that start before `wake.end`. Apple Speech accepts
   appended buffers (`SFSpeechAudioBufferRecognitionRequest.append`); Android 13+ takes an
   app audio source (`EXTRA_AUDIO_SOURCE`) but may open the microphone itself on recognizers
   that do not support it; no app-provided audio input was found for Windows
   `Windows.Media.SpeechRecognition`. Measure any such replacement with `handoff.py` first.
10. A listening cue (a sound and the indicator) starts within 100 ms of `detected_at` and
    stops when the result is final.

## The macOS App

The macOS app (see [docs/macos.md](macos.md#wake-word) for how it is used) runs the chosen
detector natively, in `src-tauri/src/dictation/wake/`:

- `rule.rs` and `detector.rs` are the Rust port of `wake.py`'s rule and "continue" capture:
  the same three-syllable rule, gap before the wake word, quiet and timeout endings, and
  the sentence taken from the same stream. `engine.rs` sets up sherpa-onnx (the `sherpa-onnx`
  crate, linked statically) exactly as `wake.py` does; the hotword is passed in memory.
- `model.rs` downloads the model files on first use, pinned to one revision of the
  upstream repository and checked against their SHA-256 (see Licenses), and writes
  `bpe.vocab` from `bpe.model` itself, so no Python step is needed on the Mac.
- `macos.rs` is the worker thread; `src-tauri/src/dictation/capture.rs` is the one owner of
  the microphone (contract item 1), shared with dictation by the microphone button, which
  holds the wake word while it runs.

Where it differs from the contract above:

- **No ring buffer** (items 3 and 9). The sentence comes from the detector's own stream, so
  nothing reads past audio; a ring is only needed by a platform recognizer replacing the
  sentence step, which the macOS app does not use.
- **Reopening** (item 5): the app does not watch for device notifications; when no audio has
  come for 3 s it opens the default input device again (every 5 s until it works), which
  covers sleep, a changed device, and a permission given back. Indices restart at zero with
  each opening.

The Rust port was checked against the prototype on the Linux server: both ran over 210
clips made with `gen_positives.py` and `gen_nearmiss.py` (a fresh synthesis of the same
set, so the numbers differ slightly from the tables above).

| Measure | Rust port |
| --- | --- |
| Same wake word decision as the prototype | 209 of 210 clips |
| Same message as the prototype (ignoring spaces and punctuation) | 206 of 210 clips |
| Miss rate | 9.3% (14 of 150) |
| Wake word end to detection, p50 / p95 | 0.51 / 0.71 s |
| Wake word alone sent | 0 of 10 |
| A sentence sent twice | 0 |
| CPU (N150, one thread), peak RSS | 15.8% of one core, 234 MB (the prototype: 16.6%, 266 MB) |

The messages that differ are single-syllable decoding differences ("yoyak" against
"yoryak"), from the Rust run feeding seven seconds of silence after each clip so that the
quiet ending fires. Near misses behave as in the prototype: the rhyming names at the start
of a sentence still wake it.

### Testing on a Mac

The measurements above ran without a Mac or a microphone. The gate still has to be met with
a real voice on an Apple Silicon Mac:

1. Install the app from the `macOS` workflow's artifact of the change (see
   [docs/macos.md](macos.md#getting-the-app)), connect it to a server, and turn on the wake
   word in Settings > General. Wait for the status line to read "Listening".
2. Hands-free path: with the window open, then closed to the menu bar, say the wake word
   and a sentence in one breath, then the wake word alone, then a sentence without it.
   Exactly one message must reach the lead chat, the first one, without its wake word.
3. Edges: Esc while the indicator shows; a reply read aloud (turn on reading aloud) that
   contains the wake word; the microphone button while listening; unplugging or switching
   the input device; sleep and wake; turning the microphone off and on again for Toodo in
   Privacy & Security. Each must leave the wake word listening again afterwards.
4. Gate: record at least 30 wake word utterances (run-on sentences included) at a desk and
   across the room, and an hour of ordinary conversation and video audio, with QuickTime
   (the app keeps no recording of what it hears); convert them to 16 kHz mono WAV and run
   `bench.py` and `handoff.py` on them as in Reproducing. Miss at most 10%, at most one false
   accept per hour, p95 latency at most 1 s, no clipped or duplicated message.
5. Cost: Activity Monitor's "% CPU" and "Memory" for Toodo while it listens, idle and
   while speaking, over a few minutes each, and whether the Mac warms up over an hour.

## Licenses

- **sherpa-onnx**: Apache-2.0.
- **The Korean zipformer model**: published by the k2-fsa project without a separate
  license; it was trained on KsponSpeech (AI Hub). AI Hub's general terms allow commercial
  and non-commercial research and development, but no dataset-specific term on
  redistributing models trained from it was found. Toodo should therefore download the model
  on first use from its upstream release, not bundle it in the installer, until the terms are
  confirmed.
- **Porcupine**: free trial for enterprise evaluation only; no personal or non-commercial
  plan at the time of writing.
- **openWakeWord**: code Apache-2.0, pretrained models CC BY-NC-SA 4.0, English only.
- **Zeroth Korean** (negatives used here): CC BY 4.0. The synthetic positives were made with
  a public TTS service for this measurement only and are not distributed.

## Limits and Next Steps

- No real voice was measured. Before the integration ships, record at least 30 real wake
  word utterances (including run-on sentences) at a desk and across the room, and an hour of
  the owner's ordinary conversation and video audio, and run `bench.py` and `handoff.py` on
  them with the same gate: miss at most 10%, at most one false accept per hour, p95 latency
  at most 1 s, no clipped or duplicated message.
- The overall miss rate (10.7%) is just over that gate, all of it from foreign-accented
  synthetic voices; the three Korean voices missed one clip in 45. The wake word rule was
  tuned on these clips ("ri" in the middle, distance 2), so a fresh set may score lower.
- If the real-voice test misses the gate, the next step is a small keyword model trained on
  the owner's recordings plus synthetic ones (sherpa-onnx KWS fine-tuning, or a
  microWakeWord-style classifier); a Porcupine plan would need a commercial agreement.
- CPU was measured on an Intel N150 under heavy load from other jobs; Apple Silicon should
  need a fraction of it, which the macOS integration should confirm, along with heat over an
  hour.

## Reproducing

From `scripts/wake-word/` with Python 3.12 and `pip install sherpa-onnx numpy soundfile
edge-tts pyarrow sentencepiece`, the model unpacked in `<model>`, and `bpe.vocab` written
from its `bpe.model` (one `piece<TAB>score` line per piece):

```sh
python gen_positives.py <samples>            # 150 clips + positives.json
python gen_nearmiss.py <samples>             # 60 clips + nearmiss.json
python prep_negatives.py <zeroth-test.parquet> <negatives>
python asr_dump.py --model-dir <model> --hotword --out hot.jsonl <samples>/clean/*.wav <samples>/nearmiss/*.wav <negatives>/*.wav
python bench.py --samples <samples> --negatives <negatives>/negatives.json --asr hot.jsonl
python kws_run.py --model-dir <model> --out kws.jsonl <same wavs>
python handoff.py --model-dir <model> --samples <samples> --strategy continue --out continue.json
python cost.py --model-dir <model> <negatives>/zeroth-00.wav
python wake.py --model-dir <model> <clip.wav>  # events with the captured message
```
