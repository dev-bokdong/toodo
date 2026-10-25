"""Score wake word rules on recognizer dumps (asr_dump.py) and keyword spotter runs (kws_run.py).

Positives: miss rate and wake-end-to-detection latency (wake end from the TTS word timings
in positives.json). Near misses: which sentences fire. Negatives: false accepts per hour.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

from wake import find_wake


def matches(tokens: list, rule: str) -> list[int]:
    """Indexes of tokens where a wake word match starts under the rule (see --rules)."""
    name, _, start_only = rule.partition("+")
    if name.startswith("phon"):
        return [first for first, _ in find_wake(tokens, int(name[4:]), bool(start_only))]
    words, owner = [], []
    for i, (tok, _) in enumerate(tokens):
        for ch in tok.replace("\u2581", ""):
            if "\uac00" <= ch <= "\ud7a3":
                words.append(ch)
                owner.append(i)
    pattern = "도리야" if name == "exact" else "도리야|돌이야"
    return sorted({owner[m.start()] for m in re.finditer(pattern, "".join(words))})


def detections(row: dict, rule: str) -> list[dict]:
    """First detection time of every distinct match (segment, token index) in a dump row."""
    seen, out = set(), []
    for snap in row["snaps"]:
        for i in matches(snap["tokens"], rule):
            key = (snap["seg"], i)
            if key not in seen:
                seen.add(key)
                out.append({"at": snap["at"], "start": snap["tokens"][i][1]})
    return out


def load(paths: list[Path]) -> dict:
    rows = {}
    for p in paths:
        for line in p.read_text().splitlines():
            row = json.loads(line)
            rows[row["file"]] = row
    return rows


def percentile(xs: list[float], q: float) -> float | None:
    if not xs:
        return None
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(round(q * (len(xs) - 1))))], 2)


def score(hits_of, positives: list, nearmiss: list, negatives: list, rows: dict) -> dict:
    missed, lat = [], []
    by_kind: dict = {}
    for clip in positives:
        hits = [h for h in hits_of(rows[clip["id"] + ".wav"]) if h["at"] >= 0.9]
        k = by_kind.setdefault(clip["kind"], [0, 0])
        k[1] += 1
        if hits:
            k[0] += 1
            if clip["wake_end"] is not None:
                lat.append(hits[0]["at"] - clip["wake_end"])
        else:
            missed.append(clip["id"])
    near = {}
    for clip in nearmiss:
        n = len(hits_of(rows[clip["id"] + ".wav"]))
        if n:
            near[clip["text"]] = near.get(clip["text"], 0) + 1
    neg_seconds = sum(rows[f]["seconds"] for f in negatives)
    neg_hits = sum(len(hits_of(rows[f])) for f in negatives)
    return {
        "positives": len(positives),
        "detected": len(positives) - len(missed),
        "miss_rate": round(len(missed) / len(positives), 3),
        "by_kind": {k: f"{v[0]}/{v[1]}" for k, v in by_kind.items()},
        "latency_p50": percentile(lat, 0.5),
        "latency_p95": percentile(lat, 0.95),
        "latency_n": len(lat),
        "nearmiss_fired": near,
        "negative_hours": round(neg_seconds / 3600, 2),
        "false_accepts": neg_hits,
        "false_accepts_per_hour": round(neg_hits / (neg_seconds / 3600), 2),
        "missed": missed,
    }


def main() -> None:
    ap = argparse.ArgumentParser(description="Score wake word rules.")
    ap.add_argument("--samples", type=Path, required=True, help="directory with positives.json and nearmiss.json")
    ap.add_argument("--negatives", type=Path, required=True, help="negatives.json from prep_negatives.py")
    ap.add_argument("--asr", type=Path, nargs="*", default=[])
    ap.add_argument("--kws", type=Path, nargs="*", default=[])
    ap.add_argument("--rules", default="exact,homophone,phon1,phon1+start,phon2+start")
    args = ap.parse_args()
    positives = json.loads((args.samples / "positives.json").read_text())
    nearmiss = json.loads((args.samples / "nearmiss.json").read_text())
    negatives = [p["file"] for p in json.loads(args.negatives.read_text())]
    report = {}
    if args.asr:
        rows = load(args.asr)
        for rule in args.rules.split(","):
            report[f"asr:{rule}"] = score(lambda r, rule=rule: detections(r, rule), positives, nearmiss, negatives, rows)
    if args.kws:
        rows = load(args.kws)
        report["kws"] = score(lambda r: r["hits"], positives, nearmiss, negatives, rows)
    print(json.dumps(report, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()
