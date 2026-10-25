"""Run the keyword spotter over WAV files; one JSON line per file with detection times."""

from __future__ import annotations

import argparse
import json
import resource
import time
from pathlib import Path

import numpy as np
import sherpa_onnx
import soundfile as sf

from wake import CHUNK, SAMPLE_RATE, model_files

KEYWORDS = ["\u2581 도 리 야", "\u2581돌 이야", "\u2581돌 이 야"]


class KeywordDetector:
    def __init__(self, model_dir: Path, score: float, threshold: float) -> None:
        self.spotter = sherpa_onnx.KeywordSpotter(
            **model_files(model_dir),
            num_threads=1,
            keywords_file=str(Path(__file__).with_name("keywords.txt")),
            num_trailing_blanks=1,
        )
        self.keywords = "/".join(f"{k} :{score} #{threshold} @도리야" for k in KEYWORDS)
        self.restart(0)

    def restart(self, at_sample: int) -> None:
        self.stream = self.spotter.create_stream(self.keywords)
        self.fed = at_sample

    def feed(self, x: np.ndarray) -> float | None:
        self.stream.accept_waveform(SAMPLE_RATE, x)
        self.fed += len(x)
        while self.spotter.is_ready(self.stream):
            self.spotter.decode_stream(self.stream)
            if self.spotter.get_result(self.stream):
                self.spotter.reset_stream(self.stream)
                return self.fed / SAMPLE_RATE
        return None


def main() -> None:
    ap = argparse.ArgumentParser(description="Run the keyword spotter over WAV files.")
    ap.add_argument("--model-dir", type=Path, required=True)
    ap.add_argument("--score", type=float, default=3.0)
    ap.add_argument("--threshold", type=float, default=0.05)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("wavs", type=Path, nargs="+")
    args = ap.parse_args()
    det = KeywordDetector(args.model_dir, args.score, args.threshold)
    audio = cpu = 0.0
    with args.out.open("w") as out:
        for wav in args.wavs:
            x, sr = sf.read(wav, dtype="float32")
            assert sr == SAMPLE_RATE and x.ndim == 1, wav
            t = time.process_time()
            det.restart(0)
            hits = []
            for i in range(0, len(x), CHUNK):
                at = det.feed(x[i : i + CHUNK])
                if at is not None:
                    hits.append({"at": round(at, 2)})
            cpu += time.process_time() - t
            audio += len(x) / SAMPLE_RATE
            out.write(json.dumps({"file": wav.name, "seconds": round(len(x) / SAMPLE_RATE, 2), "hits": hits}) + "\n")
    rss_mb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024
    print(json.dumps({"audio_seconds": round(audio, 1), "cpu_seconds": round(cpu, 1), "max_rss_mb": round(rss_mb)}))


if __name__ == "__main__":
    main()
