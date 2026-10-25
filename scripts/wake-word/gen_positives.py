"""Synthesize positive wake-word clips with edge-tts and record word timings.

Output: <out>/clean/<id>.wav (16 kHz mono) and <out>/positives.json with, per clip,
the kind (runon | runon_nospace | paused | wake_only), the sentence after the wake word,
and the wake word end time in seconds when known.
"""

import asyncio
import io
import json
import sys
from pathlib import Path

import edge_tts
import numpy as np
import soundfile as sf

SR = 16000
WAKE = "도리야"
VOICES = [
    "ko-KR-SunHiNeural",
    "ko-KR-InJoonNeural",
    "ko-KR-HyunsuMultilingualNeural",
    "en-US-AvaMultilingualNeural",
    "en-US-AndrewMultilingualNeural",
    "en-US-EmmaMultilingualNeural",
    "en-US-BrianMultilingualNeural",
    "de-DE-SeraphinaMultilingualNeural",
    "fr-FR-RemyMultilingualNeural",
    "pt-BR-ThalitaMultilingualNeural",
]
RATES = ["+0%", "+15%", "-10%", "+25%", "+5%", "-5%", "+10%", "+0%", "+20%", "-10%"]
SENTENCES = [
    "오늘 할 일 알려줘",
    "오늘 진행 중인 작업 목록을 보여줘",
    "새 작업을 만들어서 로그인 화면 버그를 고쳐 줘",
    "풀 리퀘스트가 머지되면 다음 작업을 바로 시작해",
    "어제 끝낸 작업들을 요약해서 알려줄래",
    "서버 배포가 실패한 이유를 확인하고 다시 시도해 줘",
    "음성으로 메시지를 보내는 기능을 테스트하고 있어",
    "리뷰가 필요한 작업이 있으면 먼저 알려줘",
    "이번 주 남은 일정과 우선순위를 정리해 줘",
    "지금 진행 중인 작업 상태 알려줘",
]


def resample(x: np.ndarray, sr: int) -> np.ndarray:
    if sr == SR:
        return x
    n = int(round(len(x) * SR / sr))
    return np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x).astype(np.float32)


async def tts(text: str, voice: str, rate: str):
    comm = edge_tts.Communicate(text, voice, rate=rate, boundary="WordBoundary")
    audio = bytearray()
    words = []
    async for chunk in comm.stream():
        if chunk["type"] == "audio":
            audio.extend(chunk["data"])
        elif chunk["type"] == "WordBoundary":
            words.append(
                {
                    "text": chunk["text"],
                    "start": chunk["offset"] / 1e7,
                    "end": (chunk["offset"] + chunk["duration"]) / 1e7,
                }
            )
    x, sr = sf.read(io.BytesIO(bytes(audio)), dtype="float32")
    if x.ndim > 1:
        x = x.mean(axis=1)
    return resample(x, sr), words


def pad(x: np.ndarray, before: float, after: float) -> np.ndarray:
    return np.concatenate(
        [np.zeros(int(before * SR), np.float32), x, np.zeros(int(after * SR), np.float32)]
    )


async def main(out: Path) -> None:
    (out / "clean").mkdir(parents=True, exist_ok=True)
    clips = []
    lead = 1.0  # seconds of silence before each clip

    async def save(cid, kind, voice, rate, x, sentence, wake_end):
        sf.write(out / "clean" / f"{cid}.wav", pad(x, lead, 1.5), SR, subtype="PCM_16")
        clips.append(
            {
                "id": cid,
                "kind": kind,
                "voice": voice,
                "rate": rate,
                "sentence": sentence,
                "wake_end": None if wake_end is None else round(lead + wake_end, 3),
            }
        )

    (out / "meta").mkdir(exist_ok=True)
    for vi, voice in enumerate(VOICES):
        meta = out / "meta" / f"v{vi}.json"
        if meta.exists():
            clips.extend(json.loads(meta.read_text()))
            continue
        before = len(clips)
        rate = RATES[vi]
        wake, wake_words = await tts(WAKE, voice, rate)
        await save(f"v{vi}-wake", "wake_only", voice, rate, wake, "", wake_words[-1]["end"] if wake_words else None)
        for si, sentence in enumerate(SENTENCES):
            x, words = await tts(f"{WAKE} {sentence}", voice, rate)
            first = words[0] if words else None
            wake_end = first["end"] if first and first["text"].startswith(WAKE) else None
            await save(f"v{vi}-runon-s{si}", "runon", voice, rate, x, sentence, wake_end)
        # One breath, no spaces at all.
        nospace = WAKE + SENTENCES[0].replace(" ", "")
        x, words = await tts(nospace, voice, rate)
        await save(f"v{vi}-nospace-s0", "runon_nospace", voice, rate, x, SENTENCES[0], None)
        # Wake word, a pause, then the sentence.
        for si in (0, 4, 7):
            s, _ = await tts(SENTENCES[si], voice, rate)
            gap = 0.4 + 0.3 * (si % 3)
            x = np.concatenate([wake, np.zeros(int(gap * SR), np.float32), s])
            await save(f"v{vi}-paused-s{si}", "paused", voice, rate, x, SENTENCES[si], wake_words[-1]["end"] if wake_words else None)
        meta.write_text(json.dumps(clips[before:], ensure_ascii=False))
        print(voice, "done", flush=True)
    (out / "positives.json").write_text(json.dumps(clips, ensure_ascii=False, indent=1))
    print("clips", len(clips))


if __name__ == "__main__":
    asyncio.run(main(Path(sys.argv[1])))
