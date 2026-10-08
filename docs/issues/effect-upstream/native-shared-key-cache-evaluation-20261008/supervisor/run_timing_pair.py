#!/usr/bin/env python3
"""Three rotated warm timing passes after a separately completed matched build pair."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import signal
import statistics
import subprocess
import time

BENCH = Path(__file__).resolve().parent
REVIEW = BENCH.parents[2]
PROJECT = BENCH / "project-boundary"
WORKLOAD = PROJECT / "workload.ts"
HARNESS = PROJECT / "harness.mjs"
BOUNDARY = PROJECT / "pnpm-workspace.yaml"
OUT = BENCH / "output/build-pair-retry1"
TIMING = BENCH / "output/timing-pair"
BUILD_STATUS = OUT / "build.status.json"
CHECKSUM_REFERENCE = BENCH / "checksum-reference.json"
PARENT_CLI = REVIEW / ".scratch/final-release/kiln-candidate"
CANDIDATE_CLI = REVIEW / ".scratch/source-candidates/native-intern-cache/.scratch/native-intern-cache/candidate-release-caf6e0f4-retry1/kiln-candidate"
PARENT_RECEIPT = REVIEW / ".scratch/final-release/receipt.json"
CANDIDATE_RECEIPT = CANDIDATE_CLI.with_name("receipt.json")
MANIFEST = REVIEW / ".scratch/final-release/gate-install/lib/kiln/kiln-native-provider.json"
ARCHIVE = REVIEW / ".scratch/final-release/gate-install/lib/kiln/libkiln_native_provider_ffi.a"
PARENT_BIN = OUT / "production624.bin"
CANDIDATE_BIN = OUT / "candidate.bin"
NODE = Path("~/.local/share/mise/installs/node/26.8.1/bin/node")
BUN = Path("~/.local/share/mise/installs/bun/1.4.2/bin/bun")
PINNED_PATH = f"{NODE.parent}:{BUN.parent}:/usr/bin:/bin"
TIMEOUT_S = 180
OVERALL_S = 1800
MIN_FREE = 512 * 1024**2
EXPECTED_ROWS = {
    "shared-canonical-dynamic-write": "33558527",
    "realm-local-signature-collision-write": "33558527",
    "realm-local-distinct-signature-write": "33558527",
    "warmed-own-slot-read-regression-control": "67100672",
}
EXPECTED_HASHES = {
    "workload": "eaeead3d97e32e0508321fe18e4713223fbe37a811bac15fa97a7b7114579d54",
    "harness": "b041101125e8f8e789321983bd5ec1579c3a8a8d63abf584adaae43afdcaf827",
    "boundary": "b379483d4293500af8e9d3e8d83b4b5a2ad1e844911cd7be7f7e274e4bb80666",
    "parent_cli": "c1ee6aa03d420d95bb5c52df29bfe8a770ff4816e802442b996e13ff1da085be",
    "candidate_cli": "a843877cc4bc1d6ceffa582d59f5c178e220d8781bd8a5302deb1fbbf229de1a",
    "manifest": "f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f",
    "archive": "fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720",
}


def sha(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def write_json(path: Path, value: dict) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def disk_free() -> int:
    stat = os.statvfs(TIMING)
    return stat.f_bavail * stat.f_frsize


def host_state() -> dict:
    state = {"unix_s": time.time()}
    try:
        state["loadavg"] = Path("/proc/loadavg").read_text().strip()
    except OSError:
        pass
    try:
        state["meminfo"] = {
            line.split(":", 1)[0]: line.split(":", 1)[1].strip()
            for line in Path("/proc/meminfo").read_text().splitlines()
            if line.startswith(("MemAvailable:", "SwapFree:"))
        }
    except OSError:
        pass
    return state


def parse_rows(stdout: str) -> dict:
    rows = {}
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "name" in record:
            if record["name"] in rows:
                raise RuntimeError(f"duplicate row {record['name']!r}")
            rows[record["name"]] = record
    return rows


def child_cap() -> None:
    cap = 8 * 1024**3
    resource.setrlimit(resource.RLIMIT_AS, (cap, cap))


def kill_group(pgid: int, actions: list[dict], reason: str) -> None:
    try:
        os.killpg(pgid, signal.SIGTERM)
        actions.append({"signal": "SIGTERM", "pgid": pgid, "reason": reason, "unix_s": time.time()})
    except ProcessLookupError:
        actions.append({"signal": "none", "pgid": pgid, "reason": reason, "detail": "group absent"})
        return
    until = time.monotonic() + 5
    while time.monotonic() < until:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.1)
    try:
        os.killpg(pgid, signal.SIGKILL)
        actions.append({"signal": "SIGKILL", "pgid": pgid, "reason": reason, "unix_s": time.time()})
    except ProcessLookupError:
        pass


def process_group_absent(pgid: int) -> bool:
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return True
        time.sleep(0.05)
    try:
        os.killpg(pgid, 0)
        return False
    except ProcessLookupError:
        return True


def build_inputs() -> dict:
    return {
        "parent_cli": sha(PARENT_CLI), "candidate_cli": sha(CANDIDATE_CLI),
        "parent_receipt": sha(PARENT_RECEIPT), "candidate_receipt": sha(CANDIDATE_RECEIPT),
        "parent_binary": sha(PARENT_BIN), "candidate_binary": sha(CANDIDATE_BIN),
        "manifest": sha(MANIFEST), "archive": sha(ARCHIVE),
        "workload": sha(WORKLOAD), "harness": sha(HARNESS), "boundary": sha(BOUNDARY),
        "build_status": sha(BUILD_STATUS), "checksum_reference": sha(CHECKSUM_REFERENCE),
    }


def validate_inputs() -> tuple[dict, dict]:
    for path in (WORKLOAD, HARNESS, BOUNDARY, BUILD_STATUS, CHECKSUM_REFERENCE,
                 PARENT_BIN, CANDIDATE_BIN, NODE, BUN):
        if not path.is_file():
            raise RuntimeError(f"required frozen input missing: {path}")
    status = json.loads(BUILD_STATUS.read_text())
    if status.get("status") != "terminal" or status.get("overall") != "pass" or \
            status.get("inputs_unchanged") is not True or status.get("module_c_payloads_identical") is not True:
        raise RuntimeError("matched build pair has no accepted terminal receipt")
    receipts = {
        "production624": json.loads(PARENT_RECEIPT.read_text()),
        "candidate": json.loads(CANDIDATE_RECEIPT.read_text()),
    }
    cli_hashes = {"production624": sha(PARENT_CLI), "candidate": sha(CANDIDATE_CLI)}
    expected_cli_hashes = {
        "production624": EXPECTED_HASHES["parent_cli"],
        "candidate": EXPECTED_HASHES["candidate_cli"],
    }
    if cli_hashes != expected_cli_hashes or status.get("cli_sha256") != cli_hashes:
        raise RuntimeError("CLI bytes do not match accepted staged build inputs")
    receipt_hashes = {"production624": sha(PARENT_RECEIPT), "candidate": sha(CANDIDATE_RECEIPT)}
    if status.get("cli_receipt_sha256") != receipt_hashes:
        raise RuntimeError("native build receipt does not identify the accepted CLI receipts")
    for name, receipt in receipts.items():
        if receipt.get("build_artifact_accepted") is not True:
            raise RuntimeError(f"{name} CLI receipt is not accepted")
        staged = receipt.get("staged_cli_sha256") or receipt.get("cli_sha256")
        if staged != cli_hashes[name]:
            raise RuntimeError(f"{name} CLI receipt digest mismatch")
    build_results = status.get("results", {})
    for name, binary_path in (("production624", PARENT_BIN), ("shared-cache-candidate", CANDIDATE_BIN)):
        accepted = build_results.get(name, {})
        if not binary_path.is_file() or accepted.get("binary_sha256") != sha(binary_path):
            raise RuntimeError(f"{name} native binary does not match accepted build receipt")
        if accepted.get("leader_wait_reaped") is not True or accepted.get("process_group_absent") is not True:
            raise RuntimeError(f"{name} build did not record clean process-group completion")
    reference = json.loads(CHECKSUM_REFERENCE.read_text())
    if reference.get("complete") is not True or reference.get("inputs_sha256") != {
            "workload.ts": EXPECTED_HASHES["workload"], "harness.mjs": EXPECTED_HASHES["harness"]}:
        raise RuntimeError("Node/Bun checksum reference does not match workload inputs")
    for runtime in ("node", "bun"):
        rows = reference.get("runtime_results", {}).get(runtime, {}).get("rows", [])
        observed = {row.get("name"): row.get("checksum") for row in rows}
        if observed != EXPECTED_ROWS or reference["runtime_results"][runtime].get("exact_checksum_match") is not True:
            raise RuntimeError(f"invalid {runtime} checksum control")
    hashes = build_inputs()
    expected = {
        "parent_cli": EXPECTED_HASHES["parent_cli"],
        "candidate_cli": EXPECTED_HASHES["candidate_cli"],
        "manifest": EXPECTED_HASHES["manifest"], "archive": EXPECTED_HASHES["archive"],
        "workload": EXPECTED_HASHES["workload"], "harness": EXPECTED_HASHES["harness"],
        "boundary": EXPECTED_HASHES["boundary"],
    }
    for key, value in expected.items():
        if hashes[key] != value:
            raise RuntimeError(f"input {key} digest mismatch: {hashes[key]} != {value}")
    return hashes, reference


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--start", action="store_true", help="run 3 timing passes; requires lane release")
    args = parser.parse_args()
    if not args.start:
        parser.error("no timings start without --start")
    if TIMING.exists() and any(TIMING.iterdir()):
        raise SystemExit(f"refusing to overwrite timing evidence: {TIMING}")
    TIMING.mkdir(parents=True, exist_ok=True)
    if disk_free() < MIN_FREE:
        raise SystemExit("refusing to time with less than 512 MiB free")
    hashes, reference = validate_inputs()
    runtime_versions = {}
    for label, binary in (("node", NODE), ("bun", BUN)):
        proc = subprocess.run([str(binary), "--version"], text=True, capture_output=True, check=True)
        runtime_versions[label] = {"version": proc.stdout.strip(), "binary_sha256": sha(binary)}
    if runtime_versions["node"]["version"] != "v26.8.1" or runtime_versions["bun"]["version"] != "1.4.2":
        raise SystemExit(f"unexpected Node/Bun versions: {runtime_versions}")

    commands = {
        "production624": [str(PARENT_BIN)],
        "candidate": [str(CANDIDATE_BIN)],
        "node": [str(NODE), str(WORKLOAD)],
        "bun": [str(BUN), "run", str(WORKLOAD)],
    }
    orders = [
        ["production624", "candidate", "node", "bun"],
        ["candidate", "node", "bun", "production624"],
        ["bun", "production624", "candidate", "node"],
    ]
    env = os.environ.copy()
    env.update({"PATH": PINNED_PATH, "TZ": "UTC"})
    for key in (
        "KILN_NATIVE_PROVIDER_MANIFEST", "KILN_NATIVE_NUMBER_PROVIDER_MANIFEST", "KILN_BIN",
        "KILN_NATIVE_GC_STRESS", "KILN_GC_STRESS", "KILN_REALMS", "KILN_WORKERS",
        "KILN_TYPE_PROFILE_FILE", "KILN_DEBUG_MODULE_DEMANDS", "KILN_NATIVE_TRACE_EMIT",
        "KILN_HTTP_TRACE", "KILN_HEAP_PROFILE", "KILN_HEAP_PROFILE_OBJECTS", "KILN_PARALLEL_TRACE",
    ):
        env.pop(key, None)
    env.update({"KILN_JOBS": "1", "RAYON_NUM_THREADS": "1"})
    started = time.time()
    initial = {"status": "running", "started_unix_s": started, "inputs_sha256_before": hashes,
               "runtime_versions": runtime_versions, "checksum_reference_sha256": sha(CHECKSUM_REFERENCE),
               "checksum_reference_used_for": "expected checksums only; old timing rows are ineligible",
               "commands": commands, "pass_orders": orders, "timeout_per_process_seconds": TIMEOUT_S,
               "overall_timeout_seconds": OVERALL_S, "environment": {"PATH": env["PATH"], "TZ": env["TZ"],
               "provider_overrides": "unset", "gc_stress": "unset", "realms_workers": "unset",
               "diagnostic_env": "unset", "KILN_JOBS": "1", "RAYON_NUM_THREADS": "1"},
               "host_before": host_state(),
               "scope": "matched shared-intern-cache microbenchmark only; not Effect acceptance"}
    write_json(TIMING / "timing.running.json", initial)
    records = []
    by_row = {name: {runtime: [] for runtime in commands} for name in EXPECTED_ROWS}
    deadline = time.monotonic() + OVERALL_S
    failure = None
    try:
        for pass_index, order in enumerate(orders, 1):
            for runtime in order:
                if time.monotonic() >= deadline:
                    raise TimeoutError("overall timing deadline exceeded")
                if disk_free() < MIN_FREE:
                    raise RuntimeError("disk guard triggered during timing")
                command = commands[runtime]
                logstem = f"{runtime}.pass-{pass_index}"
                stdout_path = TIMING / f"{logstem}.stdout.log"
                stderr_path = TIMING / f"{logstem}.stderr.log"
                launch = time.time()
                before_host = host_state()
                actions = []
                child = subprocess.Popen(command, cwd=PROJECT, env=env, stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE, start_new_session=True, text=True,
                                         preexec_fn=child_cap)
                pgid = os.getpgid(child.pid)
                timed_out = False
                try:
                    stdout, stderr = child.communicate(timeout=min(TIMEOUT_S, max(1, deadline-time.monotonic())))
                except subprocess.TimeoutExpired:
                    timed_out = True
                    kill_group(pgid, actions, "per-process timeout")
                    stdout, stderr = child.communicate()
                finally:
                    kill_group(pgid, actions, "final child cleanup")
                    if child.poll() is None:
                        child.wait()
                leader_reaped = child.poll() is not None
                group_absent = process_group_absent(pgid)
                stdout_path.write_text(stdout)
                stderr_path.write_text(stderr)
                rows = parse_rows(stdout)
                record = {"runtime": runtime, "pass": pass_index, "argv": command,
                          "pid": child.pid, "pgid": pgid, "exit_code": child.returncode,
                          "timed_out": timed_out, "started_unix_s": launch,
                          "elapsed_seconds": time.time()-launch, "host_before": before_host,
                          "host_after": host_state(), "process_group_cleanup": actions,
                          "leader_wait_reaped": leader_reaped, "process_group_absent": group_absent,
                          "stdout_path": str(stdout_path), "stderr_path": str(stderr_path),
                          "stdout_sha256": sha(stdout_path), "stderr_sha256": sha(stderr_path),
                          "rows": rows}
                records.append(record)
                if child.returncode != 0 or timed_out or not leader_reaped or not group_absent:
                    raise RuntimeError(f"{runtime} pass {pass_index} failed; raw logs retained")
                if set(rows) != set(EXPECTED_ROWS):
                    raise RuntimeError(f"{runtime} pass {pass_index} row inventory mismatch: {sorted(rows)}")
                for name, row in rows.items():
                    if row.get("mode") != "in-process" or not {"cold", "warm_median", "warm_best", "samples", "batch", "checksum"} <= set(row):
                        raise RuntimeError(f"{runtime}/{name} did not report in-process warm timing")
                    if row.get("checksum") != EXPECTED_ROWS[name]:
                        raise RuntimeError(f"{runtime}/{name} checksum mismatch: {row.get('checksum')}")
                    by_row[name][runtime].append(row)
                write_json(TIMING / "timing.running.json", {**initial, "records": records,
                                                              "last_runtime": runtime, "last_pass": pass_index})
    except BaseException as exc:
        failure = f"{type(exc).__name__}: {exc}"

    final_hashes = build_inputs()
    unchanged = hashes == final_hashes
    results = {}
    if failure is None:
        for name, runtimes in by_row.items():
            results[name] = {
                "expected_checksum": EXPECTED_ROWS[name],
                "runtimes": {
                    runtime: {
                        "warm_median_ms_per_iteration_median": statistics.median(x["warm_median"] for x in rows),
                        "warm_best_ms_per_iteration_median": statistics.median(x["warm_best"] for x in rows),
                        "cold_ms_median": statistics.median(x["cold"] for x in rows),
                        "passes": len(rows),
                        "sample_counts": [x["samples"] for x in rows],
                        "batches": [x["batch"] for x in rows],
                    } for runtime, rows in runtimes.items()
                },
                "checksums_match_all_runtimes_and_passes": all(
                    len(rows) == 3 and all(x["checksum"] == EXPECTED_ROWS[name] for x in rows)
                    for rows in runtimes.values()),
            }
    runtime_versions_after = {}
    for label, binary in (("node", NODE), ("bun", BUN)):
        try:
            proc = subprocess.run([str(binary), "--version"], capture_output=True, text=True,
                                  check=True, timeout=10)
            runtime_versions_after[label] = {"version": proc.stdout.strip(), "binary_sha256": sha(binary)}
        except BaseException as exc:
            runtime_versions_after[label] = {"error": f"{type(exc).__name__}: {exc}"}
    runtimes_unchanged = runtime_versions_after == runtime_versions
    if not runtimes_unchanged:
        failure = (failure + "; " if failure else "") + "Node/Bun identity changed or could not be revalidated"
    status = {
        **initial, "status": "terminal", "overall": "pass" if failure is None and unchanged else "failed",
        "failure": failure, "ended_unix_s": time.time(), "wall_seconds": time.time()-started,
        "inputs_sha256_after": final_hashes, "inputs_unchanged": unchanged,
        "runtime_versions_after": runtime_versions_after, "runtime_binaries_unchanged": runtimes_unchanged,
        "host_after": host_state(), "results": results, "records": records,
        "raw_logs_retained": True,
    }
    write_json(TIMING / "timing.status.json", status)
    write_json(TIMING / "timing.running.json", status)
    return 0 if status["overall"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
