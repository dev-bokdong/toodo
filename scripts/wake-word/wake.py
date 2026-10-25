"""Wake word prototype: streaming Korean ASR as detector, PCM ring buffer, gap-free hand-off.

One capture loop owns the microphone. Every 16 kHz mono sample goes into a ring buffer
addressed by absolute sample index and into a streaming recognizer biased toward the wake
word. A wake word is a word at the start of an utterance whose pronunciation is within a
small jamo distance of "do-ri-ya". When it is found, the sentence recognizer is fed from the
ring starting a little before the wake word, then from the live signal, so no sample is
lost or repeated at the switch. Recognized tokens that start before the wake word's end are
dropped by time, not by spelling, so a misheard wake word cannot leak into the message.

See docs/wake-word.md for the measured results and the engine/audio contract.
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

import numpy as np
import sherpa_onnx
import soundfile as sf

SAMPLE_RATE = 16000
CHUNK = SAMPLE_RATE // 10
QUIET_SECONDS = 2.0
START_TIMEOUT = 5.0
WAKE_JITTER = 0.3
PRE_ROLL = 0.6
WAKE_TAIL = 0.08
MIN_GAP = 0.5
HOTWORD = "도리야"
WAKE_JAMO = "ㄷㅗㄹㅣㅑ"
CHO = "ㄱㄲㄴㄷㄸㄹㅁㅂㅃㅅㅆㅇㅈㅉㅊㅋㅌㅍㅎ"
JUNG = "ㅏㅐㅑㅒㅓㅔㅕㅖㅗㅘㅙㅚㅛㅜㅝㅞㅟㅠㅡㅢㅣ"
JONG = ["", "ㄱ", "ㄲ", "ㄳ", "ㄴ", "ㄵ", "ㄶ", "ㄷ", "ㄹ", "ㄺ", "ㄻ", "ㄼ", "ㄽ", "ㄾ", "ㄿ", "ㅀ", "ㅁ", "ㅂ", "ㅄ", "ㅅ", "ㅆ", "ㅇ", "ㅈ", "ㅊ", "ㅋ", "ㅌ", "ㅍ", "ㅎ"]


def pronounce(text: str) -> list[str]:
    """Jamo of each syllable as spoken: a final consonant before a silent initial moves over (dol-i-ya -> do-ri-ya)."""
    syl = []
    for ch in text:
        code = ord(ch) - 0xAC00
        if 0 <= code < 11172:
            syl.append([CHO[code // 588], JUNG[code % 588 // 28], JONG[code % 28]])
    for a, b in zip(syl, syl[1:]):
        if a[2] and b[0] == "ㅇ" and len(a[2]) == 1:
            a[2], b[0] = "", a[2]
    return [(c if c != "ㅇ" else "") + v + f for c, v, f in syl]


def distance(a: str, b: str) -> int:
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        cur = [i]
        for j, cb in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = cur
    return prev[-1]


def wake_windows(tokens: list, max_distance: int) -> list[tuple[int, int, int]]:
    """(first, last, distance) for every 3-syllable window of (token, start time) pairs whose
    middle syllable is spoken "ri" and which is within max_distance jamo edits of the wake word."""
    chars, owner = [], []
    for i, (tok, _) in enumerate(tokens):
        for ch in tok.replace("\u2581", ""):
            if "\uac00" <= ch <= "\ud7a3":
                chars.append(ch)
                owner.append(i)
    out = []
    for i in range(len(chars) - 2):
        spoken_as = pronounce("".join(chars[i : i + 3]))
        if spoken_as[1].startswith("\u3139\u3163"):
            d = distance("".join(spoken_as), WAKE_JAMO)
            if d <= max_distance:
                out.append((owner[i], owner[i + 2], d))
    return out


def find_wake(tokens: list, max_distance: int = 2, start_only: bool = True) -> list[tuple[int, int]]:
    """(first, last) token indexes of wake word matches (see wake_windows) in (token, start time) pairs.

    With start_only, the previous spoken token must start MIN_GAP or more before the match.
    """
    spoken_idx = sorted({i for i, (tok, _) in enumerate(tokens) if any("\uac00" <= ch <= "\ud7a3" for ch in tok)})
    found: dict[int, int] = {}
    for first, last, _ in wake_windows(tokens, max_distance):
        found.setdefault(first, last)
    if start_only:
        def at_start(first: int) -> bool:
            k = spoken_idx.index(first)
            return k == 0 or tokens[first][1] - tokens[spoken_idx[k - 1]][1] >= MIN_GAP

        found = {f: l for f, l in found.items() if at_start(f)}
    return sorted(found.items())


def model_files(model_dir: Path) -> dict:
    return {
        "tokens": str(model_dir / "tokens.txt"),
        "encoder": str(model_dir / "encoder-epoch-99-avg-1.int8.onnx"),
        "decoder": str(model_dir / "decoder-epoch-99-avg-1.onnx"),
        "joiner": str(model_dir / "joiner-epoch-99-avg-1.int8.onnx"),
    }


def recognizer(model_dir: Path, hotword: bool = True, score: float = 2.0) -> sherpa_onnx.OnlineRecognizer:
    options = {"decoding_method": "greedy_search"}
    if hotword:
        hotwords = model_dir / "toodo-hotwords.txt"
        hotwords.write_text(HOTWORD + "\n")
        options = {
            "decoding_method": "modified_beam_search",
            "hotwords_file": str(hotwords),
            "hotwords_score": score,
            "modeling_unit": "bpe",
            "bpe_vocab": str(model_dir / "bpe.vocab"),
        }
    return sherpa_onnx.OnlineRecognizer.from_transducer(
        **model_files(model_dir),
        num_threads=1,
        sample_rate=SAMPLE_RATE,
        enable_endpoint_detection=True,
        rule1_min_trailing_silence=2.4,
        rule2_min_trailing_silence=0.8,
        rule3_min_utterance_length=20.0,
        **options,
    )


class PcmRing:
    """Fixed-size ring of float32 samples addressed by absolute sample index."""

    def __init__(self, seconds: float) -> None:
        self.buf = np.zeros(int(seconds * SAMPLE_RATE), np.float32)
        self.written = 0

    def write(self, x: np.ndarray) -> None:
        size = len(self.buf)
        if len(x) >= size:
            self.written += len(x) - size
            x = x[-size:]
        start = self.written % size
        cut = min(size - start, len(x))
        self.buf[start : start + cut] = x[:cut]
        self.buf[: len(x) - cut] = x[cut:]
        self.written += len(x)

    def read(self, start: int, end: int) -> np.ndarray:
        if start < self.written - len(self.buf) or end > self.written or start > end:
            raise ValueError(f"samples {start}..{end} not in ring ({self.written - len(self.buf)}..{self.written})")
        return self.buf[np.arange(start, end) % len(self.buf)]


@dataclass
class Wake:
    """A detected wake word; times are seconds from the start of the signal."""

    start: float
    end: float
    detected_at: float


class Detector:
    def __init__(self, rec: sherpa_onnx.OnlineRecognizer, max_distance: int = 2) -> None:
        self.rec = rec
        self.max_distance = max_distance
        self.restart(0)

    def restart(self, at_sample: int) -> None:
        self.stream = self.rec.create_stream()
        self.origin = self.fed = at_sample
        self.seen: set[int] = set()
        self.wake_tokens = 0

    def feed(self, x: np.ndarray) -> Wake | None:
        self.stream.accept_waveform(SAMPLE_RATE, x)
        self.fed += len(x)
        while self.rec.is_ready(self.stream):
            self.rec.decode_stream(self.stream)
        r = self.rec.get_result_all(self.stream)
        base = self.origin / SAMPLE_RATE
        tokens = [(t, base + at) for t, at in zip(r.tokens, r.timestamps)]
        wake = None
        for first, last in find_wake(tokens, self.max_distance):
            if first not in self.seen:
                self.seen.add(first)
                wake = Wake(tokens[first][1], tokens[last][1] + WAKE_TAIL, self.fed / SAMPLE_RATE)
                self.wake_tokens = last + 1
                return wake
        if self.rec.is_endpoint(self.stream):
            self.rec.reset(self.stream)
            self.origin = self.fed
            self.seen = set()
        return wake

    def finish_sentence(self, chunks: Iterator[np.ndarray]) -> dict:
        """Keep decoding this stream until the sentence ends; the message is what follows the wake word."""
        heard = Listening(self.fed / SAMPLE_RATE)
        for x in chunks:
            self.stream.accept_waveform(SAMPLE_RATE, x)
            self.fed += len(x)
            while self.rec.is_ready(self.stream):
                self.rec.decode_stream(self.stream)
            if heard.done(self.rec.get_result_all(self.stream).tokens[self.wake_tokens :], self.fed / SAMPLE_RATE):
                break
        r = self.rec.get_result_all(self.stream)
        return {"raw": r.text.strip(), "message": clean("".join(r.tokens[self.wake_tokens :])), "listened": round(self.fed / SAMPLE_RATE, 2)}


class Listening:
    """End of the sentence: START_TIMEOUT with nothing said after the wake word, else QUIET_SECONDS after the last word."""

    def __init__(self, now: float) -> None:
        self.began, self.last_change, self.count = now, now, 0

    def done(self, sentence_tokens: list, now: float) -> bool:
        n = sum(1 for t in sentence_tokens if spoken(t))
        if n != self.count:
            self.count, self.last_change = n, now
        if self.count == 0:
            return now - self.began >= START_TIMEOUT
        return now - self.last_change >= QUIET_SECONDS


def spoken(token: str) -> bool:
    return any(ch.isalnum() for ch in token)


def clean(text: str) -> str:
    return text.replace("\u2581", " ").strip(" ,.?!")


def transcribe(rec: sherpa_onnx.OnlineRecognizer, chunks: Iterator[np.ndarray], wake_start: float, wake_end: float) -> dict:
    """Decode replayed plus live audio until the sentence ends, then drop the wake word.

    wake_start and wake_end are the detector's wake word times relative to the first sample
    fed. The wake word is located again in this transcript by pronunciation (up to 3 jamo
    edits, its last syllable within WAKE_JITTER of the detector's), and every token through
    it is dropped; if it is not found, tokens starting before wake_end are dropped.
    """
    s = rec.create_stream()
    fed = 0
    heard = Listening(max(wake_end, 0.0))
    cut = None
    for x in chunks:
        s.accept_waveform(SAMPLE_RATE, x)
        fed += len(x)
        while rec.is_ready(s):
            rec.decode_stream(s)
        r = rec.get_result_all(s)
        tokens = list(zip(r.tokens, r.timestamps))
        cut = wake_cut(tokens, wake_start, wake_end)
        if fed / SAMPLE_RATE >= wake_end and heard.done(r.tokens[cut:], fed / SAMPLE_RATE):
            break
    s.input_finished()
    while rec.is_ready(s):
        rec.decode_stream(s)
    r = rec.get_result_all(s)
    cut = wake_cut(list(zip(r.tokens, r.timestamps)), wake_start, wake_end)
    return {"raw": r.text.strip(), "message": clean("".join(r.tokens[cut:])), "listened": round(fed / SAMPLE_RATE, 2)}


def wake_cut(tokens: list, wake_start: float, wake_end: float) -> int:
    last_token_at = wake_end - WAKE_TAIL
    near = [(d, abs(tokens[last][1] - last_token_at), last) for _, last, d in wake_windows(tokens, 3) if abs(tokens[last][1] - last_token_at) <= WAKE_JITTER]
    if near:
        return min(near)[2] + 1
    return next((i for i, (_, at) in enumerate(tokens) if at >= wake_end), len(tokens))


def capture(detector: Detector, sentence: sherpa_onnx.OnlineRecognizer, x: np.ndarray, strategy: str) -> list[dict]:
    """Run the live loop over a recorded signal and return one event per wake word.

    strategy:
      continue - keep decoding the detector's own stream; the message is the tokens after
               the wake word. No hand-off, so nothing can be lost or repeated.
      replay - feed a second recognizer from the ring, starting at the sample where the
               detector's current segment began, then live audio; drop the wake word it
               hears again at that position (transcribe). With the same model it must
               reproduce the detector's transcript, so a lost or repeated sample shows.
      replay-late - the same, starting PRE_ROLL before the wake word, as a platform
               recognizer that cannot be fed the whole segment would be.
      live   - feed it only audio captured after the detection (no ring); the naive hand-off.
    """
    ring = PcmRing(30.0)
    detector.restart(0)
    events = []
    pos = 0

    def take() -> np.ndarray:
        nonlocal pos
        chunk = x[pos : pos + CHUNK]
        ring.write(chunk)
        pos += len(chunk)
        return chunk

    while pos < len(x):
        wake = detector.feed(take())
        if wake is None:
            continue
        if strategy == "continue":

            def rest() -> Iterator[np.ndarray]:
                while pos < len(x):
                    yield take()

            result = detector.finish_sentence(rest())
            events.append({"wake_start": round(wake.start, 2), "wake_end": round(wake.end, 2), "detected_at": round(wake.detected_at, 2), **result})
            detector.restart(pos)
            continue
        floor = max(ring.written - len(ring.buf), 0)
        if strategy == "replay":
            start = max(detector.origin, floor)
        elif strategy == "replay-late":
            start = max(int((wake.start - PRE_ROLL) * SAMPLE_RATE), floor)
        else:
            start = ring.written
        head = ring.read(start, ring.written)

        def audio() -> Iterator[np.ndarray]:
            yield head
            while pos < len(x):
                yield take()

        offset = start / SAMPLE_RATE
        if strategy in ("replay", "replay-late"):
            result = transcribe(sentence, audio(), wake.start - offset, wake.end - offset)
        else:
            result = transcribe(sentence, audio(), -1.0, 0.0)
        events.append(
            {
                "wake_start": round(wake.start, 2),
                "wake_end": round(wake.end, 2),
                "detected_at": round(wake.detected_at, 2),
                "fed_from": round(start / SAMPLE_RATE, 2),
                **result,
            }
        )
        detector.restart(pos)
    return events


def main() -> None:
    ap = argparse.ArgumentParser(description="Detect the wake word in 16 kHz mono WAV files and capture the sentence after it.")
    ap.add_argument("--model-dir", type=Path, required=True)
    ap.add_argument("--strategy", choices=["continue", "replay", "replay-late", "live"], default="continue")
    ap.add_argument("--max-distance", type=int, default=2)
    ap.add_argument("wavs", type=Path, nargs="+")
    args = ap.parse_args()
    detector = Detector(recognizer(args.model_dir), args.max_distance)
    sentence = recognizer(args.model_dir)
    for wav in args.wavs:
        x, sr = sf.read(wav, dtype="float32")
        if sr != SAMPLE_RATE or x.ndim != 1:
            raise SystemExit(f"{wav}: expected {SAMPLE_RATE} Hz mono")
        print(json.dumps({"file": wav.name, "events": capture(detector, sentence, x, args.strategy)}, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()
