"""Run the streaming Korean recognizer over WAV files and dump timestamped partial results.

One JSON line per file: every time the partial hypothesis gains or changes a token, the
audio time fed so far and the tokens with their start times. Wake word rules are then
scored offline (bench.py) without decoding the audio again. CPU time per audio second is
recorded for the cost table.
"""

from __future__ import annotations

import argparse
import json
import resource
import time
from pathlib import Path

import sherpa_onnx
import soundfile as sf

from wake import CHUNK, SAMPLE_RATE, recognizer


def dump(rec: sherpa_onnx.OnlineRecognizer, path: Path) -> dict:
    x, sr = sf.read(path, dtype="float32")
    assert sr == SAMPLE_RATE and x.ndim == 1, path
    s = rec.create_stream()
    origin, fed, last = 0, 0, None
    snaps = []
    for i in range(0, len(x), CHUNK):
        s.accept_waveform(SAMPLE_RATE, x[i : i + CHUNK])
        fed += len(x[i : i + CHUNK])
        while rec.is_ready(s):
            rec.decode_stream(s)
        r = rec.get_result_all(s)
        toks = list(zip(r.tokens, (round(origin / SAMPLE_RATE + t, 2) for t in r.timestamps)))
        if toks and toks != last:
            snaps.append({"at": round(fed / SAMPLE_RATE, 2), "seg": origin, "tokens": toks})
            last = toks
        if rec.is_endpoint(s):
            rec.reset(s)
            origin, last = fed, None
    return {"file": path.name, "seconds": round(len(x) / SAMPLE_RATE, 2), "snaps": snaps}


def main() -> None:
    ap = argparse.ArgumentParser(description="Dump timestamped streaming recognizer results for WAV files.")
    ap.add_argument("--model-dir", type=Path, required=True)
    ap.add_argument("--hotword", action="store_true", help="bias decoding toward the wake word")
    ap.add_argument("--hotword-score", type=float, default=2.0)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("wavs", type=Path, nargs="+")
    args = ap.parse_args()
    rec = recognizer(args.model_dir, args.hotword, args.hotword_score)
    audio = cpu = 0.0
    with args.out.open("w") as out:
        for wav in args.wavs:
            t = time.process_time()
            row = dump(rec, wav)
            cpu += time.process_time() - t
            audio += row["seconds"]
            out.write(json.dumps(row, ensure_ascii=False) + "\n")
    rss_mb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024
    print(json.dumps({"audio_seconds": round(audio, 1), "cpu_seconds": round(cpu, 1), "max_rss_mb": round(rss_mb)}))


if __name__ == "__main__":
    main()
