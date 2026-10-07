#!/usr/bin/env python3
"""Compare process startup for the retained Full and FullGui link-only variants."""

from __future__ import annotations

import csv
import hashlib
import os
import subprocess
import sys
import time
from pathlib import Path


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("usage: launch_samples.py FULL_BIN FULLGUI_BIN OUTPUT_CSV")
    full, fullgui, output = map(Path, sys.argv[1:])
    env = os.environ.copy()
    env.pop("KILN_NATIVE_PROVIDER_MANIFEST", None)
    env.pop("KILN_NATIVE_NUMBER_PROVIDER_MANIFEST", None)

    rows = []
    binaries = {"full": full, "fullgui": fullgui}

    def run(label: str, phase: str, index: int) -> None:
        started = time.perf_counter_ns()
        result = subprocess.run(
            [str(binaries[label])],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            check=False,
            timeout=10,
        )
        elapsed_ns = time.perf_counter_ns() - started
        rows.append(
            {
                "phase": phase,
                "index": index,
                "binary": label,
                "elapsed_ms": f"{elapsed_ns / 1_000_000:.6f}",
                "exit": result.returncode,
                "stdout_sha256": sha256(result.stdout),
                "stderr_sha256": sha256(result.stderr),
                "stdout": result.stdout.decode("utf-8", errors="backslashreplace").rstrip("\n"),
            }
        )
        if result.returncode != 0:
            raise RuntimeError(f"{label} exited {result.returncode}: {result.stderr!r}")

    # Pair warmups, then alternate the starting binary to avoid a fixed order bias.
    for index in range(1, 4):
        run("full", "warmup", index)
        run("fullgui", "warmup", index)
    for index in range(1, 10):
        pair = ("full", "fullgui") if index % 2 else ("fullgui", "full")
        for label in pair:
            run(label, "sample", index)

    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=rows[0].keys())
        writer.writeheader()
        writer.writerows(rows)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
