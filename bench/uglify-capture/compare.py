#!/usr/bin/env python3
"""Compare prebuilt native-release Uglify workloads with matched source/output.

The arguments are standalone workload executables, not the kiln CLI. Build both
from the same source revision, changing only captured-record hydration in the
control, with `kiln build --node --profile native-release`. The workload times
module initialization separately, one first minify call, and a warmed 24-call
batch using the realm's host-backed performance clock.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path


REPO = Path(__file__).resolve().parents[2]
CORPUS = REPO / "conformance" / "npm-compat"
WORKLOAD = CORPUS / "benchmarks" / "uglify-capture.js"
TIME_FIELDS = ("moduleInitMs", "firstCallMs", "warmed24Ms")
DYNAMIC_CONTROL_FILE = "crates/native/kiln-backend-c/src/runtime/dynamic.c"
WORKLOAD_RELATIVE = Path("conformance/npm-compat/benchmarks/uglify-capture.js")
LOCK_RELATIVE = Path("conformance/npm-compat/package-lock.json")


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_bounded(argv: list[str], timeout: float, cwd: Path) -> tuple[bytes, bytes, float, int]:
    started = time.perf_counter()
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        partial_out = error.output or b""
        partial_err = error.stderr or b""
        # Each executable gets its own process group. Signal only that group;
        # always issue KILL after the grace interval because the leader may
        # exit on TERM while a child keeps the inherited pipes open.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=0.5)
        except subprocess.TimeoutExpired as grace:
            partial_out = grace.output or partial_out
            partial_err = grace.stderr or partial_err
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            stdout, stderr = process.communicate(timeout=1.0)
        except subprocess.TimeoutExpired as final:
            partial_out = final.output or partial_out
            partial_err = final.stderr or partial_err
            if process.poll() is None:
                process.kill()
            process.wait(timeout=1.0)
            for pipe in (process.stdout, process.stderr):
                if pipe is not None:
                    pipe.close()
            stdout, stderr = partial_out, partial_err
        raise RuntimeError(
            f"timeout after {timeout:g}s: {' '.join(argv)}\n"
            f"partial stdout: {stdout.decode('utf-8', 'replace')[-2000:]}\n"
            f"partial stderr: {stderr.decode('utf-8', 'replace')[-2000:]}"
        )
    return stdout, stderr, time.perf_counter() - started, process.returncode


def parse_result(label: str, output: bytes, stderr: bytes, code: int) -> dict[str, object]:
    if code:
        raise RuntimeError(
            f"{label} exited {code}: {stderr.decode('utf-8', 'replace')[-4000:]}"
        )
    try:
        result = json.loads(output)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"{label} did not emit benchmark JSON: {output!r}") from error
    if not isinstance(result, dict) or not isinstance(result.get("checksum"), int):
        raise RuntimeError(f"{label} emitted an invalid benchmark result: {result!r}")
    if result.get("calls") != 25:
        raise RuntimeError(f"{label} did not run the first call plus 24-call batch: {result!r}")
    for field in TIME_FIELDS:
        value = result.get(field)
        if not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
            raise RuntimeError(f"{label} has no advancing host-clock measurement in {field}: {result!r}")
    return result


def compare_sources(production: Path, control: Path) -> dict[str, object]:
    """Require identical tracked trees except the captured-record hydration file."""
    sys.path.insert(0, str(REPO))
    from bench.source_match import compare_sources as match_sources

    result = match_sources(
        production,
        control,
        allowed_differences={DYNAMIC_CONTROL_FILE},
        required_equal={
            str(WORKLOAD_RELATIVE): WORKLOAD,
            str(LOCK_RELATIVE): CORPUS / "package-lock.json",
        },
    )
    difference = result["allowed_source_differences"][DYNAMIC_CONTROL_FILE]
    return {
        "production_head": result["production_head"],
        "control_head": result["control_head"],
        "tracked_file_count": result["tracked_file_count"],
        "tracked_path_inventory_sha256": result["tracked_path_inventory_sha256"],
        "matched_source_sha256": result["matched_source_sha256"],
        "production_tracked_tree_sha256": result["production_tracked_tree_sha256"],
        "control_tracked_tree_sha256": result["control_tracked_tree_sha256"],
        "dynamic_c_sha256": {
            "production": difference["production_sha256"],
            "control": difference["control_sha256"],
        },
        "workload_sha256": result["harness_inputs_sha256"][str(WORKLOAD_RELATIVE)],
        "package_lock_sha256": result["harness_inputs_sha256"][str(LOCK_RELATIVE)],
        "only_allowed_source_difference": DYNAMIC_CONTROL_FILE,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("production", type=Path, help="prebuilt standalone production workload")
    parser.add_argument("control", type=Path, help="prebuilt standalone hydration-control workload")
    parser.add_argument("--production-source", type=Path, required=True,
                        help="production source worktree used to build its binary")
    parser.add_argument("--control-source", type=Path, required=True,
                        help="control source worktree used to build its binary")
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--rounds", type=int, default=8, help="even count of alternating timed runs")
    parser.add_argument("--timeout", type=float, default=120.0, help="per-process timeout in seconds")
    parser.add_argument("--results", type=Path, required=True, help="JSON evidence output path")
    args = parser.parse_args()
    if args.warmups < 1 or args.rounds < 4 or args.rounds % 2 or args.timeout <= 0:
        parser.error("warmups must be positive, rounds even and >= 4, and timeout positive")
    # Validate provenance before even querying Node; the evidence must describe a
    # source-matched pair, not merely two binaries that happen to agree.
    source_match = compare_sources(args.production_source, args.control_source)
    binaries = {"production": args.production.resolve(), "control": args.control.resolve()}
    for path in binaries.values():
        if not path.is_file() or not path.stat().st_mode & 0o111:
            parser.error(f"not an executable workload binary: {path}")
    if not WORKLOAD.is_file() or not (CORPUS / "node_modules" / "uglify-js").exists():
        parser.error("install conformance/npm-compat dependencies with npm ci before building workloads")

    node_version, node_error, node_elapsed, node_code = run_bounded(["node", "--version"], 10, CORPUS)
    if node_code:
        raise RuntimeError(f"cannot obtain Node version: {node_error.decode('utf-8', 'replace')}")
    node_output, node_error, node_wall, node_code = run_bounded(
        ["node", str(WORKLOAD)], args.timeout, CORPUS
    )
    reference = parse_result("Node", node_output, node_error, node_code)
    evidence: dict[str, object] = {
        "source": {
            **source_match,
        },
        "binaries": {label: {"path": str(path), "sha256": digest_file(path)}
                     for label, path in binaries.items()},
        "environment": {
            "host": platform.platform(),
            "python": platform.python_version(),
            "node": node_version.decode().strip(),
            "native_build_profile": "native-release with --node (host clock enabled)",
            "kiln_environment": {key: os.environ[key] for key in sorted(os.environ)
                                 if key.startswith("KILN_") and key in {
                                     "KILN_JOBS", "KILN_REALMS", "KILN_GC_STRESS", "KILN_IO"
                                 }},
        },
        "node_baseline": {"wall_seconds": node_wall, **reference},
        "runs": {"production": [], "control": []},
    }

    for warmup in range(args.warmups):
        warm_order = ("production", "control") if warmup % 2 == 0 else ("control", "production")
        for warm_label in warm_order:
            output, stderr, _wall, code = run_bounded(
                [str(binaries[warm_label])], args.timeout, CORPUS
            )
            result = parse_result(f"{warm_label} warmup", output, stderr, code)
            if {k: result[k] for k in ("checksum", "calls")} != {
                k: reference[k] for k in ("checksum", "calls")
            }:
                raise RuntimeError(f"{warm_label} disagrees with the Node checksum during warmup")

    for pair in range(args.rounds // 2):
        order = ("production", "control") if pair % 2 == 0 else ("control", "production")
        for label in order:
            output, stderr, wall, code = run_bounded([str(binaries[label])], args.timeout, CORPUS)
            result = parse_result(label, output, stderr, code)
            if {k: result[k] for k in ("checksum", "calls")} != {
                k: reference[k] for k in ("checksum", "calls")
            }:
                raise RuntimeError(f"{label} output diverged from Node")
            result["wallSeconds"] = wall
            evidence["runs"][label].append(result)  # type: ignore[index,union-attr]

    runs = evidence["runs"]
    p_runs = runs["production"]  # type: ignore[index]
    c_runs = runs["control"]  # type: ignore[index]
    summary = {}
    for field in (*TIME_FIELDS, "wallSeconds"):
        p_median = statistics.median(float(row[field]) for row in p_runs)
        c_median = statistics.median(float(row[field]) for row in c_runs)
        summary[field] = {"production_median": p_median, "control_median": c_median}
    summary["warmed24_speedup"] = (
        summary["warmed24Ms"]["control_median"]
        / summary["warmed24Ms"]["production_median"]
    )
    evidence["summary"] = summary
    evidence["node_version_query_wall_seconds"] = node_elapsed
    result_path = args.results.resolve()
    result_path.parent.mkdir(parents=True, exist_ok=True)
    result_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
    print(json.dumps(summary, indent=2, sort_keys=True))
    print(f"evidence: {result_path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(2)
