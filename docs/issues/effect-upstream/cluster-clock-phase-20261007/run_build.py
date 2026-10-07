#!/usr/bin/env python3
"""Bounded, source-pinned supervisor for the held phase-diagnostic build."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


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
    sent_term = sent_kill = False
    try:
        os.killpg(pgid, signal.SIGTERM)
        sent_term = True
    except ProcessLookupError:
        return {"term_sent": False, "kill_sent": False, "group_absent": True}
    deadline = time.monotonic() + grace_s
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return {"term_sent": sent_term, "kill_sent": False, "group_absent": True}
        time.sleep(0.05)
    try:
        os.killpg(pgid, signal.SIGKILL)
        sent_kill = True
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return {"term_sent": sent_term, "kill_sent": sent_kill, "group_absent": True}
        time.sleep(0.05)
    return {"term_sent": sent_term, "kill_sent": sent_kill, "group_absent": False}


def snapshot(spec: dict) -> dict:
    root = Path(spec["cwd"])
    hashes = {name: sha256(Path(path)) for name, path in spec["identity_files"].items()}
    source_head = subprocess.check_output(
        ["git", "-C", str(root / "source"), "rev-parse", "HEAD"], text=True
    ).strip()
    return {"source_head": source_head, "identity_files_sha256": hashes}


def verify_snapshot(spec: dict) -> dict:
    got = snapshot(spec)
    if got != spec["expected_snapshot"]:
        raise RuntimeError("diagnostic build inputs changed: " + json.dumps({
            "expected": spec["expected_snapshot"], "actual": got
        }, sort_keys=True))
    return got


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} build.job.json")
    job = Path(sys.argv[1]).resolve()
    spec = json.loads(job.read_text())
    running_path, status_path, log_path = map(Path, (spec["running"], spec["status"], spec["log"]))
    if any(path.exists() for path in (running_path, status_path, log_path)):
        raise SystemExit("refusing to overwrite existing diagnostic evidence")
    preflight = verify_snapshot(spec)
    if Path(spec["output"]).exists():
        raise SystemExit("refusing to overwrite existing output binary")

    started = time.time()
    child = None
    pending_signal = {"signum": None}
    exit_code = None
    timed_out = False
    interrupted_signal = None
    supervisor_error = None
    process_group_cleanup = None
    running = {
        "schema": 1,
        "state": "launching",
        "job": str(job),
        "job_sha256": sha256(job),
        "supervisor_pid": os.getpid(),
        "started_unix_s": started,
        "timeout_s": spec["timeout_s"],
        "command": spec["command"],
        "cwd": spec["cwd"],
        "environment": spec["env"],
        "preflight": preflight,
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
                exit_code = child.wait(timeout=spec["timeout_s"])
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
            process_group_cleanup = terminate_group_tail(child.pid)
        if interrupted_signal is None:
            interrupted_signal = pending_signal["signum"]

    postflight = None
    postflight_error = None
    try:
        postflight = verify_snapshot(spec)
        if postflight != preflight:
            raise RuntimeError("diagnostic source or toolchain inputs changed during build")
    except BaseException as error:
        postflight_error = repr(error)
    if interrupted_signal is None:
        interrupted_signal = pending_signal["signum"]

    output = Path(spec["output"])
    output_record = None
    if output.is_file():
        output_record = {"path": str(output), "sha256": sha256(output), "size_bytes": output.stat().st_size}
    ended = time.time()
    result = {
        **running,
        "state": "terminal",
        "exit_code": exit_code,
        "timed_out": timed_out,
        "interrupted_signal": interrupted_signal,
        "supervisor_error": supervisor_error,
        "ended_unix_s": ended,
        "wall_s": ended - started,
        "log_size_bytes": log_path.stat().st_size if log_path.exists() else None,
        "source_postflight": postflight,
        "source_postflight_error": postflight_error,
        "process_group_cleanup": process_group_cleanup,
        "output": output_record,
        "diagnostic_build_valid": (
            exit_code == 0 and not timed_out and interrupted_signal is None
            and supervisor_error is None and postflight_error is None
            and process_group_cleanup is not None and process_group_cleanup["group_absent"]
            and output_record is not None
        ),
    }
    atomic_json(status_path, result)
    return 0 if result["diagnostic_build_valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
