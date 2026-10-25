"""Check the wake-to-sentence hand-off on clips with speech after the wake word.

For every clip in positives.json with a sentence, run capture() and align the message with
the reference sentence (Hangul only). A deletion before the first aligned character means
the start of the sentence was clipped; an insertion there means part of the wake word was
sent along (duplicated).
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import soundfile as sf

from wake import Detector, capture, recognizer


def hangul(text: str) -> str:
    return "".join(ch for ch in text if "\uac00" <= ch <= "\ud7a3")


def lead(ref: str, hyp: str) -> tuple[int, int, int]:
    """(leading deletions, leading insertions, edit distance) of hyp against ref."""
    n, m = len(ref), len(hyp)
    d = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n + 1):
        d[i][0] = i
    for j in range(m + 1):
        d[0][j] = j
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (ref[i - 1] != hyp[j - 1]))
    i, j, ops = n, m, []
    while i or j:
        if i and j and d[i][j] == d[i - 1][j - 1] + (ref[i - 1] != hyp[j - 1]):
            ops.append("s" if ref[i - 1] != hyp[j - 1] else "=")
            i, j = i - 1, j - 1
        elif i and d[i][j] == d[i - 1][j] + 1:
            ops.append("d")
            i -= 1
        else:
            ops.append("i")
            j -= 1
    ops.reverse()
    k = 0
    while k < len(ops) and ops[k] in "di":
        k += 1
    head = ops[:k]
    return head.count("d"), head.count("i"), d[n][m]


def main() -> None:
    ap = argparse.ArgumentParser(description="Check the wake-to-sentence hand-off.")
    ap.add_argument("--model-dir", type=Path, required=True)
    ap.add_argument("--samples", type=Path, required=True, help="directory with positives.json and clean/")
    ap.add_argument("--strategy", choices=["continue", "replay", "replay-late", "live"], default="continue")
    ap.add_argument("--max-distance", type=int, default=2)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    detector = Detector(recognizer(args.model_dir), args.max_distance)
    sentence = recognizer(args.model_dir)
    clips = [c for c in json.loads((args.samples / "positives.json").read_text()) if c["sentence"]]
    rows = []
    for clip in clips:
        x, _ = sf.read(args.samples / "clean" / f"{clip['id']}.wav", dtype="float32")
        events = capture(detector, sentence, x, args.strategy)
        row = {"id": clip["id"], "kind": clip["kind"], "reference": clip["sentence"], "events": events}
        if events:
            ref, hyp = hangul(clip["sentence"]), hangul(events[0]["message"])
            deleted, inserted, edits = lead(ref, hyp)
            row |= {"clipped": deleted, "duplicated": inserted, "cer": round(edits / len(ref), 3)}
        rows.append(row)
        print(json.dumps(row, ensure_ascii=False), flush=True)
    done = [r for r in rows if r["events"]]
    summary = {
        "strategy": args.strategy,
        "clips": len(rows),
        "detected": len(done),
        "extra_events": sum(len(r["events"]) - 1 for r in done),
        "clipped": sum(r["clipped"] > 0 for r in done),
        "duplicated": sum(r["duplicated"] > 0 for r in done),
        "mean_cer": round(sum(r["cer"] for r in done) / max(len(done), 1), 3),
    }
    args.out.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=1))
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
