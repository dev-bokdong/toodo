import io
import json
import random
import sys
from pathlib import Path

import numpy as np
import pyarrow.parquet as pq
import soundfile as sf

SR = 16000
src, out = Path(sys.argv[1]), Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
table = pq.read_table(src)
print(table.schema)
rows = table.to_pylist()
random.seed(309)
chunks, texts, total, part, parts = [], [], 0.0, 0, []


def flush():
    global chunks, texts, part
    if not chunks:
        return
    x = np.concatenate(chunks)
    name = f"zeroth-{part:02d}.wav"
    sf.write(out / name, x, SR, subtype="PCM_16")
    parts.append({"file": name, "seconds": round(len(x) / SR, 1), "texts": texts})
    chunks, texts, part = [], [], part + 1


for row in rows:
    audio = row["audio"]
    x, sr = sf.read(io.BytesIO(audio["bytes"]), dtype="float32")
    if x.ndim > 1:
        x = x.mean(axis=1)
    if sr != SR:
        n = int(round(len(x) * SR / sr))
        x = np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x).astype(np.float32)
    chunks.append(x)
    chunks.append(np.zeros(int(random.uniform(0.3, 1.5) * SR), np.float32))
    texts.append(row.get("text", ""))
    total += len(x) / SR
    if sum(len(c) for c in chunks) > 600 * SR:
        flush()
flush()
(out / "negatives.json").write_text(json.dumps(parts, ensure_ascii=False, indent=1))
print("utterances", len(rows), "speech seconds", round(total), "files", len(parts))
