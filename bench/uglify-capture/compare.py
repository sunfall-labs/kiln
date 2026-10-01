#!/usr/bin/env python3
"""Matched native timing for production vs captured-record hydration control.

Build each binary from the same source revision, changing only the dynamic
captured-record hydration implementation in the control. Run with npm deps
installed in conformance/npm-compat; warmups prime each binary's artifact
cache before alternating measured invocations. Stdout is checked every time.
"""

from __future__ import annotations

import argparse
import statistics
import subprocess
import sys
import time
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
CORPUS = REPO / "conformance" / "npm-compat"
WORKLOAD = CORPUS / "benchmarks" / "uglify-capture.js"


def invoke(binary: Path, timeout: float) -> tuple[float, bytes]:
    started = time.perf_counter()
    try:
        result = subprocess.run(
            [str(binary), "--one-shot", "run", "--native", str(WORKLOAD)],
            cwd=CORPUS,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise RuntimeError(f"{binary} exceeded {timeout:g}s") from error
    elapsed = time.perf_counter() - started
    if result.returncode:
        detail = result.stderr.decode("utf-8", "replace")[-4000:]
        raise RuntimeError(f"{binary} exited {result.returncode}:\n{detail}")
    return elapsed, result.stdout


def node_baseline(timeout: float) -> bytes:
    try:
        result = subprocess.run(
            ["node", str(WORKLOAD)],
            cwd=CORPUS,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"Node baseline failed: {error}") from error
    if result.returncode:
        detail = result.stderr.decode("utf-8", "replace")[-4000:]
        raise RuntimeError(f"Node baseline exited {result.returncode}:\n{detail}")
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("production", type=Path, help="release kiln binary with borrowed views")
    parser.add_argument("control", type=Path, help="release kiln binary with per-call hydration")
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--rounds", type=int, default=9)
    parser.add_argument("--timeout", type=float, default=120.0)
    args = parser.parse_args()
    if args.warmups < 1 or args.rounds < 3 or args.timeout <= 0:
        parser.error("use at least one warmup, three rounds, and a positive timeout")
    production, control = args.production.resolve(), args.control.resolve()
    for binary in (production, control):
        if not binary.is_file() or not binary.stat().st_mode & 0o111:
            parser.error(f"not an executable binary: {binary}")
    if not WORKLOAD.is_file() or not (CORPUS / "node_modules" / "uglify-js").exists():
        parser.error("install conformance/npm-compat dependencies with npm ci first")

    reference = node_baseline(args.timeout)
    samples: dict[str, list[float]] = {"production": [], "control": []}
    binaries = {"production": production, "control": control}

    def run(label: str, phase: str, index: int) -> bool:
        elapsed, output = invoke(binaries[label], args.timeout)
        if output != reference:
            sys.stderr.write(
                f"stdout mismatch: {label} {phase} {index}; expected={reference!r}, got={output!r}\n"
            )
            return False
        if phase == "measure":
            samples[label].append(elapsed)
        print(f"{phase} {index:02d} {label} {elapsed:.6f}s", flush=True)
        return True

    # Prime both artifact caches and establish the exact output before timing.
    for index in range(args.warmups):
        for label in ("production", "control"):
            if not run(label, "warm", index):
                return 2

    # AB / BA pairs counterbalance thermal and background drift.
    for index in range(args.rounds):
        order = ("production", "control") if index % 2 == 0 else ("control", "production")
        for label in order:
            if not run(label, "measure", index):
                return 2

    p50 = statistics.median(samples["production"])
    c50 = statistics.median(samples["control"])
    print(f"median production={p50:.6f}s control={c50:.6f}s speedup={c50 / p50:.3f}x")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(2)
