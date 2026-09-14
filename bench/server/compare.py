#!/usr/bin/env python3
"""Portable paired HTTP benchmark runner with owned process-group cleanup."""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def port_free(port: int) -> bool:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.bind(("127.0.0.1", port))
        return True
    except OSError:
        return False
    finally:
        probe.close()


def stop_process(process: subprocess.Popen[bytes], timeout: float = 2.0) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=timeout)


def wait_ready(port: int, process: subprocess.Popen[bytes], timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    url = f"http://127.0.0.1:{port}/"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=0.25):
                return
        except Exception:
            time.sleep(0.1)
    raise RuntimeError(f"server did not answer on port {port} within {timeout:g}s")


def runtime_command(runtime: str, variant: dict[str, Path], workload: str, sources: Path) -> list[str]:
    script = sources / ("serve.mjs" if workload == "hello" else "app.mjs")
    if runtime == "kiln":
        return [str(variant["hello" if workload == "hello" else "app"])]
    if runtime == "node":
        return ["node", str(script)]
    if runtime == "bun":
        return ["bun", "run", str(script)]
    if runtime == "deno":
        return ["deno", "run", "--allow-net", "--quiet", str(script)]
    raise ValueError(f"unknown runtime {runtime}")


def run_client(driver: Path, duration: int, connections: int, port: int, workload: str,
               timeout: float) -> tuple[dict, str, int]:
    command = ["node", str(driver), str(duration), str(connections), str(port)]
    if driver.name == "drive.mjs":
        command.append(workload)
    client = subprocess.run(command, capture_output=True, timeout=timeout, check=False)
    rows = [line for line in client.stdout.decode("utf-8", "replace").splitlines() if line.strip()]
    if not rows:
        raise RuntimeError(f"client produced no JSON (status {client.returncode})")
    try:
        report = json.loads(rows[-1])
    except json.JSONDecodeError as error:
        raise RuntimeError(f"client JSON was invalid: {rows[-1]!r}") from error
    report["exit_status"] = client.returncode
    return report, client.stderr.decode("utf-8", "replace"), client.returncode


def main() -> int:
    if len(sys.argv) not in (3, 6) or (len(sys.argv) == 6 and sys.argv[3] != "--baseline"):
        print("usage: compare.sh <candidate-hello> <candidate-app> [--baseline <hello> <app>]", file=sys.stderr)
        return 2
    candidate = {"hello": Path(sys.argv[1]).resolve(), "app": Path(sys.argv[2]).resolve()}
    variants = [("candidate", candidate)]
    if len(sys.argv) == 6:
        variants.insert(0, ("baseline", {"hello": Path(sys.argv[4]).resolve(), "app": Path(sys.argv[5]).resolve()}))
    for _, paths in variants:
        for path in paths.values():
            if not path.is_file() or not os.access(path, os.X_OK):
                raise SystemExit(f"binary is not executable: {path}")

    passes = int(os.environ.get("PASSES", "3")); processes = int(os.environ.get("PROCS", "4"))
    connections = int(os.environ.get("CONNS", "16")); duration = int(os.environ.get("DURATION_MS", "5000"))
    port = int(os.environ.get("PORT", "18088")); runtimes = os.environ.get("RUNTIMES", "kiln node bun deno").split()
    workloads = os.environ.get("WORKLOADS", "hello mixed static-file json-echo-1k json-echo-64k chunked-stream large-upload connection-churn fs-small-reads fs-large-read fs-stat-storm").split()
    result_root = Path(os.environ.get("RESULTS_DIR", ROOT / ".scratch" / f"server-compare-{int(time.time())}-{os.getpid()}"))
    result_root.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="kiln-server-sources-") as temporary:
        sources = Path(temporary)
        for name in ("serve.mjs", "app.mjs"):
            (sources / name).write_text((HERE / name).read_text().replace("18088", str(port)))
        metadata = {"git_commit": subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip(),
                    "serve_source_sha256": sha256(HERE / "serve.mjs"), "app_source_sha256": sha256(HERE / "app.mjs"),
                    "driver_source_sha256": sha256(HERE / "drive.mjs"), "drive_mix_source_sha256": sha256(HERE / "drive-mix.mjs"),
                    "variants": {label: {kind: sha256(path) for kind, path in paths.items()} for label, paths in variants},
                    "passes": passes, "processes": processes, "connections": connections, "duration_ms": duration,
                    "port": port, "runtimes": runtimes, "workloads": workloads}
        (result_root / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        for pass_number in range(1, passes + 1):
            for workload in workloads:
                driver = HERE / ("drive-mix.mjs" if workload == "mixed" else "drive.mjs")
                for runtime in runtimes:
                    if runtime != "kiln" and shutil.which(runtime) is None:
                        print(f"pass={pass_number} workload={workload:<18} runtime={runtime:<4} skipped (not on PATH)")
                        continue
                    if not port_free(port):
                        raise RuntimeError(f"port {port} is already occupied")
                    for label, variant in variants:
                        process = subprocess.Popen(runtime_command(runtime, variant, workload, sources),
                                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
                        run_dir = result_root / f"pass-{pass_number}-{label}-{runtime}-{workload}"; run_dir.mkdir()
                        try:
                            wait_ready(port, process)
                            reports = []
                            for index in range(processes):
                                report, stderr, status = run_client(driver, duration, connections, port, workload,
                                                                    max(10.0, duration / 1000 + 5.0))
                                (run_dir / f"client-{index + 1}.json").write_text(json.dumps(report, indent=2) + "\n")
                                if stderr: (run_dir / f"client-{index + 1}.stderr").write_text(stderr)
                                reports.append(report)
                                if status != 0 or report.get("errors", 0) or report.get("wrong", 0):
                                    raise RuntimeError(f"client validation failed: {report}")
                            aggregate = {"pass": pass_number, "label": label, "runtime": runtime, "workload": workload,
                                         "reports": reports, "rps": sum(row["rps"] for row in reports)}
                            (run_dir / "aggregate.json").write_text(json.dumps(aggregate, indent=2) + "\n")
                            print(json.dumps(aggregate, separators=(",", ":")))
                        finally:
                            stop_process(process)
                            if not port_free(port): raise RuntimeError(f"owned server left port {port} occupied")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"compare.py: {error}", file=sys.stderr); raise SystemExit(1)
