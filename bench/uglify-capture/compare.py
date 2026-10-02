#!/usr/bin/env python3
"""Compare prebuilt native-release Uglify workloads with matched source/output.

The arguments are standalone workload executables, not the kiln CLI. By
default, compare borrowed-view capture against captured-record hydration. The
optional dictionary-index mode compares the reviewed five-file index change
with its correct pre-index binary. The workload times module initialization,
one first minify call, and a warmed 24-call batch using the host-backed clock.
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
DICTIONARY_CONTROL_FILES = {
    "crates/native/kiln-backend-c/src/runtime/heap.c": {
        "production": "ef1e82fcbdbe2bb9aff467f243caa33fe1bd3cb6157352805624702c756f761a",
        "control": "1417c74b45150dcefc9a755be363eda32d57375bb35059a70d2d9dfc4b332ca2",
    },
    "crates/native/kiln-backend-c/src/runtime/prelude_types.h": {
        "production": "d198e378bde807a6278c2b0bb1e8ce4a57b3f7bac778232b5172c3b91e793438",
        "control": "c6b435768f176af1bdb6faa705c20dfa145c2ce5b1ab0cef6d52cdbcbfc398f3",
    },
    "crates/native/kiln-backend-c/src/runtime/properties.c": {
        "production": "3758a4fabc90ae39af900678010ca06c96c8e3f2a28606ddff2094682068ee39",
        "control": "92b0dcacc82b088435b9f8d4bf878fe984085888fccaa0ed632238a96f62d6eb",
    },
    "crates/native/kiln-backend-c/src/runtime/shapes.c": {
        "production": "b5ab5846e0c2e5ee89e9c50c24998d54df9d954a4884ba7d9a3dfcf5d4279e7a",
        "control": "4a4d668c2e1c80bf89e619ef18afc5606adf238d61d698556f1a729f5f8bfd5d",
    },
    "crates/native/kiln-backend-c/src/runtime/spread.c": {
        "production": "5985283d17da7a3fc49ace8f1dedf139595d761727fcecb595f0fc9a501b2306",
        "control": "de1a318fd4482ff06977f008075800da683a64afaf21a6ad3505cfd76a8e6827",
    },
}
DICTIONARY_PRODUCTION_HEAD = "8b8931ae"
DICTIONARY_CONTROL_HEAD = "9e775ce4"
WORKLOAD_RELATIVE = Path("conformance/npm-compat/benchmarks/uglify-capture.js")
LOCK_RELATIVE = Path("conformance/npm-compat/package-lock.json")


def digest_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_bounded(argv: list[str], timeout: float, cwd: Path,
                timeout_prefix: Path | None = None) -> tuple[bytes, bytes, float, int]:
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
        if timeout_prefix is not None:
            timeout_prefix.with_suffix(".stdout.log").write_bytes(stdout)
            timeout_prefix.with_suffix(".stderr.log").write_bytes(stderr)
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
    if not isinstance(result, dict) or type(result.get("checksum")) is not int:
        raise RuntimeError(f"{label} emitted an invalid benchmark result: {result!r}")
    if result.get("calls") != 25:
        raise RuntimeError(f"{label} did not run the first call plus 24-call batch: {result!r}")
    for field in TIME_FIELDS:
        value = result.get(field)
        if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
            raise RuntimeError(f"{label} has no advancing host-clock measurement in {field}: {result!r}")
    return result


def compare_sources(production: Path, control: Path, control_kind: str) -> dict[str, object]:
    """Require identical tracked trees except the reviewed control's exact file set."""
    sys.path.insert(0, str(REPO))
    from bench.source_match import compare_sources as match_sources

    if control_kind == "capture":
        allowed = {DYNAMIC_CONTROL_FILE}
    elif control_kind == "dictionary-index":
        allowed = set(DICTIONARY_CONTROL_FILES)
    else:
        raise RuntimeError(f"unknown Uglify control kind: {control_kind}")
    result = match_sources(
        production,
        control,
        allowed_differences=allowed,
        required_equal={
            str(WORKLOAD_RELATIVE): WORKLOAD,
            str(LOCK_RELATIVE): CORPUS / "package-lock.json",
        },
    )
    if control_kind == "capture":
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

    actual = result["allowed_source_differences"]
    if set(actual) != set(DICTIONARY_CONTROL_FILES):
        raise RuntimeError(f"dictionary control files differ from reviewed set: {sorted(actual)}")
    for relative, expected in DICTIONARY_CONTROL_FILES.items():
        pair = actual[relative]
        if pair["production_sha256"] != expected["production"] or pair["control_sha256"] != expected["control"]:
            raise RuntimeError(f"dictionary source delta is not the reviewed implementation: {relative}: {pair}")
    digest_input = "".join(
        f"{path}\0{DICTIONARY_CONTROL_FILES[path]['production']}\0{DICTIONARY_CONTROL_FILES[path]['control']}\n"
        for path in sorted(DICTIONARY_CONTROL_FILES)
    )
    return {
        "production_head": result["production_head"],
        "control_head": result["control_head"],
        "tracked_file_count": result["tracked_file_count"],
        "tracked_path_inventory_sha256": result["tracked_path_inventory_sha256"],
        "matched_source_sha256": result["matched_source_sha256"],
        "production_tracked_tree_sha256": result["production_tracked_tree_sha256"],
        "control_tracked_tree_sha256": result["control_tracked_tree_sha256"],
        "workload_sha256": result["harness_inputs_sha256"][str(WORKLOAD_RELATIVE)],
        "package_lock_sha256": result["harness_inputs_sha256"][str(LOCK_RELATIVE)],
        "only_allowed_source_differences": actual,
        "reviewed_delta_sha256": hashlib.sha256(digest_input.encode()).hexdigest(),
        "reviewed_production_commit": DICTIONARY_PRODUCTION_HEAD,
        "reviewed_control_commit": DICTIONARY_CONTROL_HEAD,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("production", type=Path, help="prebuilt standalone production workload")
    parser.add_argument("control", type=Path, help="prebuilt standalone control workload")
    parser.add_argument("--production-source", type=Path, required=True,
                        help="production source worktree used to build its binary")
    parser.add_argument("--control-source", type=Path, required=True,
                        help="control source worktree used to build its binary")
    parser.add_argument("--control-kind", choices=("capture", "dictionary-index"), default="capture",
                        help="reviewed control delta; default preserves the captured-record comparison")
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--rounds", type=int, default=8, help="even count of alternating timed runs")
    parser.add_argument("--timeout", type=float, default=120.0, help="per-process timeout in seconds")
    parser.add_argument("--results", type=Path, required=True, help="JSON evidence output path")
    args = parser.parse_args()
    if args.warmups < 1 or args.rounds < 4 or args.rounds % 2 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("warmups must be positive, rounds even and >= 4, and timeout positive and finite")

    # Validate provenance before querying Node or launching a workload.
    source_match = compare_sources(args.production_source, args.control_source, args.control_kind)
    binaries = {"production": args.production.resolve(), "control": args.control.resolve()}
    for path in binaries.values():
        if not path.is_file() or not path.stat().st_mode & 0o111:
            parser.error(f"not an executable workload binary: {path}")
    if not WORKLOAD.is_file() or not (CORPUS / "node_modules" / "uglify-js").exists():
        parser.error("install conformance/npm-compat dependencies with npm ci before building workloads")

    result_path = args.results.resolve()
    partial_path = result_path.with_suffix(result_path.suffix + ".partial.json")
    raw_dir = result_path.parent / (result_path.stem + "-raw")
    raw_dir.mkdir(parents=True, exist_ok=True)
    evidence: dict[str, object] = {
        "source": source_match,
        "binaries": {label: {"path": str(path), "sha256": digest_file(path)}
                     for label, path in binaries.items()},
        "environment": {
            "host": platform.platform(),
            "python": platform.python_version(),
            "native_build_profile": "native-release with --node (host clock enabled)",
            "kiln_environment": {key: os.environ[key] for key in sorted(os.environ)
                                 if key.startswith("KILN_") and key in {
                                     "KILN_JOBS", "KILN_REALMS", "KILN_GC_STRESS", "KILN_IO"
                                 }},
        },
        "runs": {"production": [], "control": []},
    }
    if args.control_kind != "capture":
        evidence["control_kind"] = args.control_kind
    partial: dict[str, object] = {"status": "running", "control_kind": args.control_kind,
                                  "evidence": evidence, "raw_logs": str(raw_dir), "events": []}

    def persist_partial() -> None:
        partial_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = partial_path.with_suffix(partial_path.suffix + ".tmp")
        temporary.write_text(json.dumps(partial, indent=2, sort_keys=True) + "\n")
        temporary.replace(partial_path)

    def execute(label: str, index: int, argv: list[str]) -> tuple[bytes, bytes, float, int]:
        prefix = raw_dir / f"{label}-{index:02d}"
        partial["active"] = {"label": label, "index": index, "log_prefix": str(prefix)}
        persist_partial()
        output, stderr, wall, code = run_bounded(argv, args.timeout, CORPUS, prefix)
        prefix.with_suffix(".stdout.log").write_bytes(output)
        prefix.with_suffix(".stderr.log").write_bytes(stderr)
        partial["events"].append({"label": label, "index": index, "wall_seconds": wall,
                                  "stdout": str(prefix.with_suffix(".stdout.log")),
                                  "stderr": str(prefix.with_suffix(".stderr.log")),
                                  "exit_code": code})
        partial.pop("active", None)
        persist_partial()
        return output, stderr, wall, code

    persist_partial()
    try:
        node_version, node_error, node_elapsed, node_code = execute("node-version", 0, ["node", "--version"])
        if node_code:
            raise RuntimeError(f"cannot obtain Node version: {node_error.decode('utf-8', 'replace')}")
        evidence["environment"]["node"] = node_version.decode().strip()
        node_output, node_error, node_wall, node_code = execute("node-reference", 0, ["node", str(WORKLOAD)])
        reference = parse_result("Node", node_output, node_error, node_code)
        evidence["node_baseline"] = {"wall_seconds": node_wall, **reference}
        evidence["node_version_query_wall_seconds"] = node_elapsed
        persist_partial()

        for warmup in range(args.warmups):
            warm_order = ("production", "control") if warmup % 2 == 0 else ("control", "production")
            for warm_label in warm_order:
                output, stderr, _wall, code = execute(
                    f"{warm_label}-warmup", warmup, [str(binaries[warm_label])]
                )
                result = parse_result(f"{warm_label} warmup", output, stderr, code)
                if {key: result[key] for key in ("checksum", "calls")} != {
                    key: reference[key] for key in ("checksum", "calls")
                }:
                    raise RuntimeError(f"{warm_label} disagrees with Node checksum during warmup")
                partial["events"].append({"warmup_result": warm_label, "index": warmup,
                                          "checksum": result["checksum"], "calls": result["calls"]})
                persist_partial()

        for pair in range(args.rounds // 2):
            order = ("production", "control") if pair % 2 == 0 else ("control", "production")
            for label in order:
                output, stderr, wall, code = execute(
                    label, pair * 2 + (0 if label == "production" else 1), [str(binaries[label])]
                )
                result = parse_result(label, output, stderr, code)
                if {key: result[key] for key in ("checksum", "calls")} != {
                    key: reference[key] for key in ("checksum", "calls")
                }:
                    raise RuntimeError(f"{label} output diverged from Node")
                result["wallSeconds"] = wall
                evidence["runs"][label].append(result)
                partial["events"].append({"timed_result": label, "pair": pair,
                                          "checksum": result["checksum"], "calls": result["calls"]})
                persist_partial()

        runs = evidence["runs"]
        p_runs = runs["production"]
        c_runs = runs["control"]
        summary = {}
        for field in (*TIME_FIELDS, "wallSeconds"):
            p_median = statistics.median(float(row[field]) for row in p_runs)
            c_median = statistics.median(float(row[field]) for row in c_runs)
            summary[field] = {"production_median": p_median, "control_median": c_median}
        summary["warmed24_speedup"] = (
            summary["warmed24Ms"]["control_median"] / summary["warmed24Ms"]["production_median"]
        )
        evidence["summary"] = summary
        result_path.parent.mkdir(parents=True, exist_ok=True)
        result_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
        partial["status"] = "complete"
        partial["summary"] = summary
        partial_path.unlink(missing_ok=True)
        print(json.dumps(summary, indent=2, sort_keys=True))
        print(f"evidence: {result_path}")
        print(f"raw logs: {raw_dir}")
        return 0
    except BaseException as error:
        partial["status"] = "failed"
        partial["failure"] = f"{type(error).__name__}: {error}"
        persist_partial()
        raise


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(2)
