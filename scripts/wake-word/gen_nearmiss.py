"""Synthesize near-miss negatives: sentences that sound like the wake word but are not a call."""

import asyncio
import json
import sys
from pathlib import Path

import soundfile as sf

from gen_positives import SR, VOICES, pad, tts

NEAR_MISS = [
    "도리어 일이 더 복잡해졌어",
    "아이가 도리도리를 하면서 웃었어",
    "그건 사람으로서 지켜야 할 도리야",
    "부모님께 효도하는 게 자식의 도리야",
    "저기 오리야 연못에 오리가 있어",
    "이게 무슨 소리야 갑자기 시끄럽네",
    "열쇠 고리야 그거 내 거야",
    "우리야 뭐 괜찮지",
    "토리야 이리 와 산책 가자",
    "도리토스 한 봉지만 사다 줘",
    "노리야 밥 먹었어",
    "도라야 오늘 날씨 어때",
    "그 돌이야 아까 말한 돌",
    "드라마 이름이 도리였나",
    "도로가 막혀서 늦을 것 같아",
]


async def main(out: Path) -> None:
    (out / "nearmiss").mkdir(parents=True, exist_ok=True)
    clips = []
    for vi, voice in enumerate(VOICES[:4]):
        for ti, text in enumerate(NEAR_MISS):
            x, _ = await tts(text, voice, "+0%")
            cid = f"v{vi}-near-{ti}"
            sf.write(out / "nearmiss" / f"{cid}.wav", pad(x, 1.0, 1.0), SR, subtype="PCM_16")
            clips.append({"id": cid, "voice": voice, "text": text})
    (out / "nearmiss.json").write_text(json.dumps(clips, ensure_ascii=False, indent=1))
    print("clips", len(clips))


if __name__ == "__main__":
    asyncio.run(main(Path(sys.argv[1])))
