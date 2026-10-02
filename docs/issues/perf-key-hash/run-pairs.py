#!/usr/bin/env python3
"""Run three paired, in-process warmed dictionary-key-hash samples."""
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
WORKLOAD = ROOT / "docs/issues/perf-key-hash/workload.mjs"
HARNESS = ROOT / "bench/harness.mjs"
EVIDENCE = ROOT / "target/npm-compat-evidence/key-hash-pair"
CONTROL = EVIDENCE / "control.bin"
CANDIDATE = EVIDENCE / "candidate.bin"
EXPECTED_CHECKSUM = "8421376"
EXPECTED_ITERATIONS = 65_536
SCHEDULE = [
    ("control", 1), ("candidate", 1),
    ("candidate", 2), ("control", 2),
    ("control", 3), ("candidate", 3),
]


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    runs_dir = EVIDENCE / "runs"
    runs_dir.mkdir(exist_ok=True)
    sources = {
        "workload": {"path": str(WORKLOAD), "sha256": sha(WORKLOAD)},
        "harness": {"path": str(HARNESS), "sha256": sha(HARNESS)},
    }
    binaries = {"control": {"path": str(CONTROL), "sha256": sha(CONTROL)},
                "candidate": {"path": str(CANDIDATE), "sha256": sha(CANDIDATE)}}
    environment = os.environ.copy()
    environment.pop("NODE_ENV", None)
    environment["TZ"] = "UTC"
    records = []
    for role, pair in SCHEDULE:
        binary = Path(binaries[role]["path"])
        name = f"{role}-{pair}"
        out_path = runs_dir / f"{name}.stdout"
        err_path = runs_dir / f"{name}.stderr"
        started = time.monotonic()
        with out_path.open("wb") as stdout, err_path.open("wb") as stderr:
            process = subprocess.run([str(binary), str(WORKLOAD)], cwd=ROOT, env=environment,
                                     stdout=stdout, stderr=stderr, timeout=30, check=False)
        elapsed = time.monotonic() - started
        lines = [line for line in out_path.read_text(errors="replace").splitlines() if line.strip()]
        if process.returncode != 0 or len(lines) != 1:
            raise RuntimeError(f"{name}: exit={process.returncode}, nonempty_stdout_lines={len(lines)}; see {out_path} and {err_path}")
        result = json.loads(lines[0])
        if result.get("name") != "dictionary-key-hash" or result.get("mode") != "in-process":
            raise RuntimeError(f"{name}: expected in-process dictionary timing, got {result!r}")
        if result.get("checksum") != EXPECTED_CHECKSUM:
            raise RuntimeError(f"{name}: checksum {result.get('checksum')} != {EXPECTED_CHECKSUM}")
        for field in ("warm_median", "warm_best"):
            value = result.get(field)
            if not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
                raise RuntimeError(f"{name}: invalid warmed timing {field}={value!r}")
        if result.get("samples", 0) < 1 or result.get("batch", 0) < 1:
            raise RuntimeError(f"{name}: no warmed timing samples: {result!r}")
        record = {"run": name, "role": role, "pair": pair, "exit": process.returncode,
                  "external_elapsed_seconds_including_startup": round(elapsed, 6),
                  "stdout": str(out_path), "stderr": str(err_path), "result": result}
        records.append(record)
        (EVIDENCE / "partial-results.json").write_text(json.dumps({
            "state": "running", "expected_checksum": EXPECTED_CHECKSUM,
            "iterations_per_workload": EXPECTED_ITERATIONS, "sources": sources,
            "binaries": binaries, "node_env": None, "timezone": "UTC",
            "timing_units": "harness warm_median/warm_best are milliseconds per workload invocation",
            "schedule": [f"{kind}-{index}" for kind, index in SCHEDULE], "runs": records,
        }, indent=2) + "\n")
    medians = {}
    for role in ("control", "candidate"):
        values = sorted(r["result"]["warm_median"] for r in records if r["role"] == role)
        medians[role] = values[len(values) // 2]
    evidence = {"state": "complete", "expected_checksum": EXPECTED_CHECKSUM,
                "iterations_per_workload": EXPECTED_ITERATIONS, "sources": sources,
                "binaries": binaries, "node_env": None, "timezone": "UTC",
                "timing_units": "milliseconds per workload invocation; warm_median from bench/harness.mjs",
                "schedule": [f"{kind}-{index}" for kind, index in SCHEDULE],
                "runs": records, "median_of_three_run_medians_ms": medians,
                "control_over_candidate_speedup": medians["control"] / medians["candidate"]}
    (EVIDENCE / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({"median_of_three_run_medians_ms": medians,
                      "control_over_candidate_speedup": evidence["control_over_candidate_speedup"]}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except BaseException as error:
        print(f"measurement failed: {error}", file=sys.stderr)
        raise
