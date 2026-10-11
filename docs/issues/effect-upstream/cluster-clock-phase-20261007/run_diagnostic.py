#!/usr/bin/env python3
"""Run one modified Cluster diagnostic copy with the original 47-case harness limits."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

PREFIX = "KILN_TESTCLOCK_PHASE_DIAGNOSTIC "


class StopRequested(Exception):
    def __init__(self, signum: int):
        self.signum = signum


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def terminate_group_tail(pgid: int, grace_s: float = 1.0) -> dict:
    term_sent = kill_sent = False
    try:
        os.killpg(pgid, signal.SIGTERM)
        term_sent = True
    except ProcessLookupError:
        return {"term_sent": False, "kill_sent": False, "group_absent": True}
    deadline = time.monotonic() + grace_s
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return {"term_sent": term_sent, "kill_sent": False, "group_absent": True}
        time.sleep(0.05)
    try:
        os.killpg(pgid, signal.SIGKILL)
        kill_sent = True
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return {"term_sent": term_sent, "kill_sent": kill_sent, "group_absent": True}
        time.sleep(0.05)
    return {"term_sent": term_sent, "kill_sent": kill_sent, "group_absent": False}


def source_snapshot(spec: dict) -> dict:
    workspace = Path(spec["cwd"])
    source = workspace / "source"
    head = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    return {
        "source_head": head,
        "binary_sha256": sha256(Path(spec["binary"])),
        "build_status_sha256": sha256(Path(spec["build_status"])),
        "runner_sha256": sha256(Path(spec["upstream_runner"])),
        "supervisor_sha256": sha256(Path(spec["supervisor"])),
        "harness_sha256": sha256(Path(spec["harness"])),
        "inventory_sha256": sha256(Path(spec["expected_inventory"])),
        "test_sha256": sha256(source / "packages/effect/test/cluster/ClusterWorkflowEngine.test.ts"),
        "testclock_sha256": sha256(source / "packages/effect/src/testing/TestClock.ts"),
        "overlay_sha256": sha256(Path(spec["workspace_path_overlay"])),
        "provider_manifest_sha256": sha256(Path(spec["provider_manifest"])),
        "provider_archive_sha256": sha256(Path(spec["provider_manifest"]).parent / "libkiln_native_provider_ffi.a"),
    }


def parse_phase_events(log: Path, target: Path) -> list[dict]:
    events = []
    with log.open(errors="replace") as stream, target.open("x") as output:
        for line in stream:
            if not line.startswith(PREFIX):
                continue
            raw = line[len(PREFIX):].strip()
            output.write(raw + "\n")
            try:
                events.append(json.loads(raw))
            except json.JSONDecodeError as error:
                events.append({"kind": "malformed_phase_event", "error": str(error), "raw": raw})
    return events


def load_upstream_records(runner: Path, log: Path) -> dict:
    spec = importlib.util.spec_from_file_location("effect_upstream_runner", runner)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load pinned upstream result parser")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.records(log)


def verify_preflight(job_path: Path, spec: dict) -> dict:
    build_status = json.loads(Path(spec["build_status"]).read_text())
    if not build_status.get("diagnostic_build_valid"):
        raise RuntimeError("diagnostic build receipt is not valid")
    if build_status.get("output", {}).get("sha256") != spec["binary_sha256"]:
        raise RuntimeError("diagnostic binary does not match build receipt")
    inventory = json.loads(Path(spec["expected_inventory"]).read_text())
    if inventory.get("count") != spec["expected_case_count"] or len(inventory.get("inventory", [])) != spec["expected_case_count"]:
        raise RuntimeError("frozen original inventory does not contain expected 47 rows")
    if inventory.get("source_sha256") != spec["original_test_source_sha256"]:
        raise RuntimeError("frozen expected inventory is not tied to the original test source")
    if inventory.get("harness_sha256") != spec["harness_sha256"]:
        raise RuntimeError("inventory harness digest differs")
    snap = source_snapshot(spec)
    expected = {
        "source_head": spec["original_source_pin"],
        "binary_sha256": spec["binary_sha256"],
        "build_status_sha256": spec["build_status_sha256"],
        "runner_sha256": spec["upstream_runner_sha256"],
        "supervisor_sha256": spec["supervisor_sha256"],
        "harness_sha256": spec["harness_sha256"],
        "inventory_sha256": spec["expected_inventory_sha256"],
        "test_sha256": spec["diagnostic_test_source_sha256"],
        "testclock_sha256": spec["diagnostic_testclock_source_sha256"],
        "overlay_sha256": spec["workspace_path_overlay_sha256"],
        "provider_manifest_sha256": spec["provider_manifest_sha256"],
        "provider_archive_sha256": spec["provider_archive_sha256"],
    }
    if snap != expected:
        raise RuntimeError("diagnostic runtime preflight mismatch: " + json.dumps({"expected": expected, "actual": snap}, sort_keys=True))
    return {"source_snapshot": snap, "expected_inventory": inventory["inventory"], "build_status_sha256": sha256(Path(spec["build_status"]))}


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} run.job.json")
    job_path = Path(sys.argv[1]).resolve()
    spec = json.loads(job_path.read_text())
    log_path, running_path, status_path, events_path = map(Path, (spec["log"], spec["running"], spec["status"], spec["phase_events"]))
    if any(path.exists() for path in (log_path, running_path, status_path, events_path)):
        raise SystemExit("refusing to overwrite earlier diagnostic execution evidence")
    preflight = verify_preflight(job_path, spec)
    started = time.time()
    child = None
    pending_signal = {"signum": None}
    exit_code = None
    timed_out = False
    interrupted_signal = None
    supervisor_error = None
    cleanup = None
    running = {
        "schema": 1,
        "state": "launching",
        "job": str(job_path),
        "job_sha256": sha256(job_path),
        "supervisor_pid": os.getpid(),
        "started_unix_s": started,
        "timeout_s": spec["outer_timeout_s"],
        "case_deadline_ms": spec["original_case_timeout_ms"],
        "command": spec["command"],
        "cwd": spec["cwd"],
        "environment": spec["env"],
        "preflight": preflight,
        "claim": "modified-source diagnostic only; not acceptance",
    }
    atomic_json(running_path, running)

    def request_stop(signum, _frame):
        pending_signal["signum"] = signum
        if child is None or child.poll() is None:
            raise StopRequested(signum)

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    environment = os.environ.copy()
    environment.update(spec["env"])
    try:
        with log_path.open("xb") as log:
            child = subprocess.Popen(
                spec["command"], cwd=spec["cwd"], env=environment,
                stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            running.update({"state": "running", "child_pid": child.pid, "process_group_id": child.pid})
            atomic_json(running_path, running)
            try:
                exit_code = child.wait(timeout=spec["outer_timeout_s"])
            except subprocess.TimeoutExpired:
                timed_out = True
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    exit_code = child.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    exit_code = child.wait()
            except StopRequested as stop:
                interrupted_signal = stop.signum
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    exit_code = child.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    exit_code = child.wait()
    except StopRequested as stop:
        interrupted_signal = stop.signum
        if child is not None and child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            exit_code = child.wait()
    except BaseException as error:
        supervisor_error = repr(error)
        if child is not None and child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            exit_code = child.wait()
    finally:
        if child is not None:
            if child.poll() is None:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    exit_code = child.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    exit_code = child.wait()
            cleanup = terminate_group_tail(child.pid)
        if interrupted_signal is None:
            interrupted_signal = pending_signal["signum"]

    postflight = None
    postflight_error = None
    try:
        postflight = source_snapshot(spec)
        if postflight != preflight["source_snapshot"]:
            raise RuntimeError("diagnostic inputs changed during execution")
    except BaseException as error:
        postflight_error = repr(error)
    if interrupted_signal is None:
        interrupted_signal = pending_signal["signum"]

    records = None
    parse_error = None
    events = []
    try:
        records = load_upstream_records(Path(spec["upstream_runner"]), log_path)
        events = parse_phase_events(log_path, events_path)
    except BaseException as error:
        parse_error = repr(error)
        if log_path.exists() and not events_path.exists():
            events_path.write_text("")
    expected = preflight["expected_inventory"]
    actual = records.get("inventory") if records else None
    inventory_match = actual == expected
    outcomes = records.get("cases", []) if records else []
    result = {
        **running,
        "state": "terminal",
        "exit_code": exit_code,
        "timed_out_outer": timed_out,
        "interrupted_signal": interrupted_signal,
        "supervisor_error": supervisor_error,
        "ended_unix_s": time.time(),
        "wall_s": time.time() - started,
        "log_size_bytes": log_path.stat().st_size if log_path.exists() else None,
        "phase_event_file": str(events_path),
        "phase_event_count": len(events),
        "phase_events": events,
        "case_records_complete": bool(records and records.get("complete")),
        "case_summary": records.get("summary") if records else None,
        "case_counts_by_outcome": {
            outcome: sum(row.get("outcome") == outcome for row in outcomes)
            for outcome in ("pass", "fail", "skip")
        },
        "inventory_exactly_matches_original_ids_and_modes": inventory_match,
        "case_outcomes": outcomes,
        "parse_error": parse_error,
        "source_postflight": postflight,
        "source_postflight_error": postflight_error,
        "process_group_cleanup": cleanup,
        "phase_instrumentation_scope_caveat": (
            "Counters are global to this diagnostic process but activated only by the target test. "
            "If its effect continues after harness timeout, later TestClock activity may share the active phase; "
            "require a finally marker and bound interpretation to the target case deadline."
        ),
        "diagnostic_run_valid": (
            exit_code is not None and not timed_out and interrupted_signal is None
            and supervisor_error is None and postflight_error is None and parse_error is None
            and cleanup is not None and cleanup["group_absent"] and inventory_match
            and bool(records and records.get("complete"))
            and len(outcomes) == spec["expected_case_count"]
        ),
        "acceptance_claim": False,
    }
    atomic_json(status_path, result)
    return 0 if result["diagnostic_run_valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
