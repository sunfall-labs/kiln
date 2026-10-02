#!/usr/bin/env python3
"""Run source-matched, warmed npm throughput probes against Node and optional controls."""

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

ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "conformance/npm-compat"
PROBES = CORPUS / "probes"
WORKLOADS = {
    "bcryptjs": PROBES / "bcryptjs-throughput.mjs",
    "numeric": PROBES / "numeric-bitwise-throughput.mjs",
}
CONTROL_FILES = {"bcryptjs": "crates/native/kiln-backend-c/src/runtime/boxed.c"}
REVIEWED_BCRYPT_BOXED_SHA256 = {
    "candidate": "0cda4d5bd0cad42f30a6d657c7afca09ea630a187e9f1a91d533fc85e8daa5d1",
    "control": "7c8ff400d7c4291a9b53501816422b189472d4e3e73c9695e14fea5ef450bc4b",
}
sys.path.insert(0, str(ROOT))
from bench.source_match import compare_sources, digest_entry, tracked_paths, worktree_entry  # noqa: E402


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def source_fingerprint(root: Path, workload: Path) -> dict[str, object]:
    root = root.resolve()
    paths = tracked_paths(root)
    tree = hashlib.sha256()
    for name in paths:
        digest_entry(tree, name, worktree_entry(root, name))
    relative = str(workload.relative_to(ROOT))
    if (root / relative).read_bytes() != workload.read_bytes():
        raise RuntimeError(f"source workload differs from harness: {relative}")
    lock = "conformance/npm-compat/package-lock.json"
    if (root / lock).read_bytes() != (CORPUS / "package-lock.json").read_bytes():
        raise RuntimeError("source package-lock.json differs from harness corpus")
    harness = "bench/harness.mjs"
    if (root / harness).read_bytes() != (ROOT / harness).read_bytes():
        raise RuntimeError("source bench/harness.mjs differs from harness corpus")
    return {"head": subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip(),
            "tracked_file_count": len(paths), "tracked_path_inventory_sha256": hashlib.sha256(
                "\0".join(paths).encode()).hexdigest(), "tracked_tree_sha256": tree.hexdigest(),
            "workload_sha256": sha256(workload), "package_lock_sha256": sha256(CORPUS / "package-lock.json")}


def run(argv: list[str], timeout: float, cwd: Path, timeout_prefix: Path | None = None) -> tuple[bytes, bytes, float, int]:
    start = time.perf_counter()
    process = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    try:
        out, err = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired as error:
        out = error.output or b""
        err = error.stderr or b""
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=0.5)
        except subprocess.TimeoutExpired as grace:
            out = grace.output or out
            err = grace.stderr or err
        # Always kill the group after the grace period: the leader may have
        # exited while a child still holds either inherited pipe open.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            out, err = process.communicate(timeout=1.0)
        except subprocess.TimeoutExpired as final:
            out = final.output or out
            err = final.stderr or err
            if process.poll() is None:
                process.kill()
            process.wait(timeout=1.0)
            for pipe in (process.stdout, process.stderr):
                if pipe is not None:
                    pipe.close()
        if timeout_prefix is not None:
            timeout_prefix.with_suffix(".stdout.log").write_bytes(out)
            timeout_prefix.with_suffix(".stderr.log").write_bytes(err)
        raise RuntimeError(f"timeout after {timeout:g}s: {' '.join(argv)}\n"
                           f"stdout={out[-2000:]!r}\nstderr={err[-2000:]!r}")
    return out, err, time.perf_counter() - start, process.returncode


def parse(label: str, out: bytes, err: bytes, code: int, expected_name: str) -> dict[str, object]:
    if code:
        raise RuntimeError(f"{label} exited {code}: {err.decode('utf-8', 'replace')[-4000:]}")
    try:
        value = json.loads(out)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RuntimeError(f"{label} did not emit one JSON result: {out!r}") from error
    if not isinstance(value, dict) or value.get("name") != expected_name:
        raise RuntimeError(f"{label} emitted an unexpected result: {value!r}")
    if value.get("mode") != "in-process":
        raise RuntimeError(f"{label} is not an in-process timed run: {value!r}")
    if not isinstance(value.get("checksum"), str) or not value["checksum"]:
        raise RuntimeError(f"{label} has no checksum: {value!r}")
    for field in ("warm_median", "warm_best"):
        number = value.get(field)
        if type(number) not in (int, float) or not math.isfinite(number) or number <= 0:
            raise RuntimeError(f"{label} has no positive finite {field}: {value!r}")
    if type(value.get("batch")) is not int or value["batch"] <= 0 or \
            type(value.get("samples")) is not int or value["samples"] <= 0:
        raise RuntimeError(f"{label} has invalid sample metadata: {value!r}")
    return value


def save_log(directory: Path, label: str, index: int, out: bytes, err: bytes) -> None:
    stem = directory / f"{label}-{index:02d}"
    stem.with_suffix(".stdout.log").write_bytes(out)
    stem.with_suffix(".stderr.log").write_bytes(err)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("workload", choices=WORKLOADS)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--candidate-source", type=Path, required=True)
    parser.add_argument("--control", type=Path)
    parser.add_argument("--control-source", type=Path)
    parser.add_argument("--rounds", type=int, default=8, help="even count of alternating timed runs")
    parser.add_argument("--warmups", type=int, default=2, help="warmup launches per binary")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()
    if args.rounds < 4 or args.rounds % 2 or args.warmups < 1 or not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("rounds must be even and >= 4, warmups positive, timeout positive and finite")
    workload = WORKLOADS[args.workload]
    expected_name = "npm-bcryptjs-fixed-salt" if args.workload == "bcryptjs" else "npm-numeric-bitwise-loop"
    candidates = {"candidate": args.candidate.resolve()}
    if (args.control is None) != (args.control_source is None):
        parser.error("--control and --control-source must be provided together")
    if args.workload == "bcryptjs" and args.control is None:
        parser.error("bcryptjs comparison requires its typed-array-read control")
    if args.control:
        candidates["control"] = args.control.resolve()
    for path in candidates.values():
        if not path.is_file() or not path.stat().st_mode & 0o111:
            parser.error(f"not an executable workload binary: {path}")
    if not workload.is_file():
        parser.error(f"missing workload: {workload}")
    if args.workload == "bcryptjs" and not (CORPUS / "node_modules/bcryptjs").exists():
        parser.error("install conformance/npm-compat dependencies with npm ci first")
    if args.control:
        if args.workload not in CONTROL_FILES:
            parser.error("this workload has no reviewed optimization control")
        source = compare_sources(
            args.candidate_source, args.control_source,
            allowed_differences={CONTROL_FILES[args.workload]},
            required_equal={str(workload.relative_to(ROOT)): workload,
                            "bench/harness.mjs": ROOT / "bench/harness.mjs",
                            "conformance/npm-compat/package-lock.json": CORPUS / "package-lock.json"},
        )
        diff_path = CONTROL_FILES[args.workload]
        actual_hashes = {
            "candidate": source["allowed_source_differences"][diff_path]["production_sha256"],
            "control": source["allowed_source_differences"][diff_path]["control_sha256"],
        }
        if actual_hashes != REVIEWED_BCRYPT_BOXED_SHA256:
            raise RuntimeError(f"reviewed bcrypt optimization source delta changed: {actual_hashes}")
        source["reviewed_source_delta_sha256"] = hashlib.sha256(
            (actual_hashes["candidate"] + "\0" + actual_hashes["control"]).encode()
        ).hexdigest()
    else:
        source = source_fingerprint(args.candidate_source, workload)
        source["comparison_scope"] = "candidate-vs-Node; no optimization control or gain claim"

    result_path = args.results.resolve()
    result_dir = result_path.parent / (result_path.stem + "-logs")
    result_dir.mkdir(parents=True, exist_ok=True)
    binaries = {label: {"path": str(path), "sha256": sha256(path)} for label, path in candidates.items()}
    evidence: dict[str, object] = {
        "status": "running", "workload": args.workload,
        "timing_kind": "in-process warm_median milliseconds per workload call",
        "source": source, "binaries": binaries,
        "environment": {"host": platform.platform(), "python": platform.python_version(),
                        "node": subprocess.check_output(["node", "--version"], text=True).strip()},
        "runs": {label: [] for label in candidates},
        "warmup_runs": {label: [] for label in candidates},
        "raw_logs": str(result_dir), "active_stage": "Node reference",
    }

    def persist() -> None:
        result_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = result_path.with_suffix(result_path.suffix + ".tmp")
        temporary.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")
        temporary.replace(result_path)

    def invoke(label: str, index: int, argv: list[str], timeout: float) -> tuple[bytes, bytes, float, int]:
        evidence["active_stage"] = f"{label} launch {index}"
        prefix = result_dir / f"{label}-{index:02d}"
        evidence["active_log_prefix"] = str(prefix)
        persist()
        out, err, wall, code = run(argv, timeout, CORPUS, prefix)
        save_log(result_dir, label, index, out, err)
        evidence.setdefault("completed_raw_logs", []).append({
            "label": label, "index": index,
            "stdout": str(prefix.with_suffix(".stdout.log")),
            "stderr": str(prefix.with_suffix(".stderr.log")),
        })
        persist()
        return out, err, wall, code

    persist()
    try:
        node_out, node_err, node_wall, node_code = invoke("node", 0, ["node", str(workload)], args.timeout)
        reference = parse("Node", node_out, node_err, node_code, expected_name)
        expected_checksum = reference["checksum"]
        evidence["node_reference"] = {"wall_seconds_including_harness": node_wall, **reference}
        persist()

        bcrypt_oracle = None
        if args.workload == "bcryptjs":
            evidence["active_stage"] = "bcrypt deterministic Node oracle"
            persist()
            oracle_code = ('const b=require("bcryptjs"); const p="kiln-npm-compat-benchmark", '
                           's="$2a$08$N9qo8uLOickgx2ZMRZoMye"; const h=b.hashSync(p,s); '
                           'console.log(JSON.stringify({hash:h,match:b.compareSync(p,h),'
                           'reject:b.compareSync("wrong password",h)===false,rounds:b.getRounds(h)}))')
            oracle_out, oracle_err, _, oracle_status = invoke(
                "bcrypt-node-oracle", 0, ["node", "-e", oracle_code], 15)
            bcrypt_oracle = json.loads(oracle_out) if not oracle_status else None
            if oracle_status or bcrypt_oracle != {
                    "hash": "$2a$08$N9qo8uLOickgx2ZMRZoMyeoxUxJ05MRWm4bW3Giar6lxjRF5NYzvS",
                    "match": True, "reject": True, "rounds": 8}:
                raise RuntimeError(f"bcrypt deterministic Node oracle failed: {oracle_out!r} {oracle_err!r}")
            evidence["bcrypt_node_hash_oracle"] = bcrypt_oracle
            persist()

        def execute(label: str, phase_index: int, phase: str) -> dict[str, object]:
            out, err, wall, code = invoke(label, phase_index, [str(candidates[label])], args.timeout)
            result = parse(f"{label} {phase}", out, err, code, expected_name)
            if result["checksum"] != expected_checksum:
                raise RuntimeError(f"{label} checksum {result['checksum']} differs from Node {expected_checksum}")
            result["process_wall_seconds"] = wall
            return result

        labels = list(candidates)
        for idx in range(args.warmups):
            order = labels if idx % 2 == 0 else list(reversed(labels))
            for label in order:
                warm = execute(label, -(idx + 1), "warmup")
                evidence["warmup_runs"][label].append(warm)
                persist()
        for pair in range(args.rounds // 2):
            order = labels if pair % 2 == 0 else list(reversed(labels))
            for label in order:
                run_index = len(evidence["runs"][label]) + 1
                row = execute(label, run_index, "timed")
                evidence["runs"][label].append(row)
                persist()
        medians = {label: statistics.median(row["warm_median"] for row in evidence["runs"][label])
                   for label in labels}
        summary: dict[str, object] = {"warm_median_ms_per_call": medians}
        if "control" in medians:
            summary["control_over_candidate_speedup"] = medians["control"] / medians["candidate"]
        else:
            summary["gain_claim"] = "none; candidate correctness and warmed runtime measured against Node only"
        evidence["summary"] = summary
        evidence["status"] = "complete"
        evidence.pop("active_stage", None)
        persist()
        print(json.dumps(summary, indent=2, sort_keys=True))
        print(f"evidence: {result_path}")
        print(f"raw logs: {result_dir}")
        return 0
    except BaseException as error:
        evidence["status"] = "failed"
        evidence["failure"] = f"{type(error).__name__}: {error}"
        persist()
        raise


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"benchmark failed: {error}", file=sys.stderr)
        raise SystemExit(2)
