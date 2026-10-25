"""Measure the detector's CPU (1 core = 100%) and peak RSS on speech or on near-silence.

Audio is fed in 100 ms chunks as fast as possible; CPU seconds divided by audio seconds is
the share of one core the detector needs in real time.
"""

from __future__ import annotations

import argparse
import json
import resource
import time
from pathlib import Path

import numpy as np
import soundfile as sf

from wake import CHUNK, SAMPLE_RATE, Detector, recognizer


def main() -> None:
    ap = argparse.ArgumentParser(description="Measure detector CPU and RSS.")
    ap.add_argument("--model-dir", type=Path, required=True)
    ap.add_argument("--seconds", type=float, default=180.0)
    ap.add_argument("--no-hotword", action="store_true")
    ap.add_argument("--silence", action="store_true", help="feed low-level noise instead of the WAV")
    ap.add_argument("wav", type=Path)
    args = ap.parse_args()
    n = int(args.seconds * SAMPLE_RATE)
    if args.silence:
        x = (np.random.default_rng(0).standard_normal(n) * 1e-3).astype(np.float32)
    else:
        x, sr = sf.read(args.wav, dtype="float32", frames=n)
        assert sr == SAMPLE_RATE
    load_start = time.process_time()
    det = Detector(recognizer(args.model_dir, hotword=not args.no_hotword))
    load_cpu = time.process_time() - load_start
    rss_loaded = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024
    start, wall = time.process_time(), time.monotonic()
    for i in range(0, len(x), CHUNK):
        det.feed(x[i : i + CHUNK])
    cpu = time.process_time() - start
    audio = len(x) / SAMPLE_RATE
    print(
        json.dumps(
            {
                "input": "silence" if args.silence else args.wav.name,
                "hotword": not args.no_hotword,
                "audio_seconds": round(audio, 1),
                "cpu_seconds": round(cpu, 2),
                "cpu_percent_of_one_core": round(100 * cpu / audio, 1),
                "wall_seconds": round(time.monotonic() - wall, 1),
                "load_cpu_seconds": round(load_cpu, 1),
                "rss_after_load_mb": round(rss_loaded),
                "max_rss_mb": round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024),
            }
        )
    )


if __name__ == "__main__":
    main()
