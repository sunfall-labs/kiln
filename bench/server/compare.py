#!/usr/bin/env python3
"""Run paired HTTP measurements with auditable metadata and owned processes."""
from __future__ import annotations
import hashlib, json, os, shutil, signal, socket, subprocess, sys, tempfile, time, urllib.request
from pathlib import Path
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]

def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""): digest.update(chunk)
    return digest.hexdigest()

def port_free(port: int) -> bool:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", port)); return True
    except OSError:
        return False
    finally:
        probe.close()

def group_exists(process: subprocess.Popen[bytes]) -> bool:
    try:
        os.killpg(process.pid, 0); return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True

def stop_process(process: subprocess.Popen[bytes], timeout: float = 2.0) -> None:
    """Terminate the entire owned group, even when its leader already exited."""
    try: os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError: pass
    deadline = time.monotonic() + timeout
    while group_exists(process) and time.monotonic() < deadline:
        process.poll()  # reap the leader while descendants drain
        time.sleep(0.02)
    if group_exists(process):
        try: os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError: pass
    try: process.wait(timeout=max(0.1, timeout))
    except subprocess.TimeoutExpired: process.poll()

def wait_ready(port: int, process: subprocess.Popen[bytes], timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    url = f"http://127.0.0.1:{port}/"
    while time.monotonic() < deadline:
        if process.poll() is not None: raise RuntimeError(f"server exited with status {process.returncode}")
        try:
            with urllib.request.urlopen(url, timeout=0.25): return
        except Exception: time.sleep(0.1)
    raise RuntimeError(f"server did not answer on port {port} within {timeout:g}s")

def runtime_command(runtime: str, variant: dict[str, Path], workload: str, sources: Path) -> list[str]:
    simple_server = workload in ("hello", "pipeline")
    script = sources / ("serve.mjs" if simple_server else "app.mjs")
    if runtime == "kiln": return [str(variant["hello" if simple_server else "app"])]
    if runtime == "node": return ["node", str(script)]
    if runtime == "bun": return ["bun", "run", str(script)]
    if runtime == "deno": return ["deno", "run", "--allow-net", "--quiet", str(script)]
    raise ValueError(f"unknown runtime {runtime}")

def client_command(node_bin: str, driver: Path, duration: int, connections: int, port: int, workload: str) -> list[str]:
    command = [node_bin, str(driver), str(duration), str(connections), str(port)]
    if driver.name == "drive.mjs": command.append(workload)
    return command

def decode_client(process: subprocess.Popen[bytes], stdout: bytes, stderr: bytes) -> tuple[dict, str, int]:
    rows = [line for line in stdout.decode("utf-8", "replace").splitlines() if line.strip()]
    if not rows: raise RuntimeError(f"client produced no JSON (status {process.returncode})")
    try: report = json.loads(rows[-1])
    except json.JSONDecodeError as error: raise RuntimeError(f"client JSON was invalid: {rows[-1]!r}") from error
    report["exit_status"] = process.returncode
    return report, stderr.decode("utf-8", "replace"), process.returncode

def run_clients(driver: Path, duration: int, connections: int, port: int, workload: str, processes: int,
                timeout: float, node_bin: str = "node", output_dir: Path | None = None) -> list[tuple[dict, str, int]]:
    if processes <= 0: raise ValueError("processes must be positive")
    clients = []
    captured = set()
    results = []
    try:
        for _ in range(processes):
            clients.append(subprocess.Popen(client_command(node_bin, driver, duration, connections, port, workload),
                                            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE))
        for index, client in enumerate(clients, 1):
            try:
                stdout, stderr = client.communicate(timeout=timeout)
                captured.add(index)
            except subprocess.TimeoutExpired:
                client.kill()
                stdout, stderr = client.communicate()
                captured.add(index)
                if output_dir is not None:
                    (output_dir / f"client-{index}.stdout").write_bytes(stdout)
                    (output_dir / f"client-{index}.stderr").write_bytes(stderr)
                raise RuntimeError("client timed out")
            if output_dir is not None:
                (output_dir / f"client-{index}.stdout").write_bytes(stdout)
                (output_dir / f"client-{index}.stderr").write_bytes(stderr)
            result = decode_client(client, stdout, stderr)
            if result[0].get("requests", 0) <= 0:
                raise RuntimeError(f"client completed without requests: {result[0]}")
            results.append(result)
        return results
    finally:
        for index, client in enumerate(clients, 1):
            if client.poll() is None:
                client.kill()
            if index in captured:
                continue
            try:
                stdout, stderr = client.communicate(timeout=1)
            except subprocess.TimeoutExpired:
                client.kill()
                stdout, stderr = client.communicate()
            if output_dir is not None:
                (output_dir / f"client-{index}.stdout").write_bytes(stdout)
                (output_dir / f"client-{index}.stderr").write_bytes(stderr)


def runtime_version(runtime: str, variant: dict[str, Path]) -> str:
    if runtime == "kiln":
        return os.environ.get("KILN_VERSION", "not-probed (server binary)")
    command = [runtime, "--version"]
    try: result = subprocess.run(command, capture_output=True, text=True, timeout=5, check=False)
    except (OSError, subprocess.TimeoutExpired) as error: return f"unavailable: {error}"
    output = (result.stdout or result.stderr).strip().splitlines()
    return output[0] if output else f"exit {result.returncode}"

def positive_setting(name: str, default: str) -> int:
    try: value = int(os.environ.get(name, default))
    except ValueError as error: raise ValueError(f"{name} must be a positive integer") from error
    if value <= 0: raise ValueError(f"{name} must be a positive integer")
    return value

def paired_variant_order(variants: list[tuple[str, dict[str, Path]]], pass_number: int) -> list[tuple[str, dict[str, Path]]]:
    """Alternate A/B order between passes to avoid a fixed warmup/order advantage."""
    return variants if pass_number % 2 else list(reversed(variants))

def runtime_variant_order(variants: list[tuple[str, dict[str, Path]]], runtime: str, pass_number: int) -> list[tuple[str, dict[str, Path]]]:
    if runtime == "kiln":
        return paired_variant_order(variants, pass_number)
    return [variants[-1]]

def main() -> int:
    if len(sys.argv) not in (3, 6) or (len(sys.argv) == 6 and sys.argv[3] != "--baseline"):
        print("usage: compare.sh <candidate-hello> <candidate-app> [--baseline <hello> <app>]", file=sys.stderr); return 2
    candidate = {"hello": Path(sys.argv[1]).resolve(), "app": Path(sys.argv[2]).resolve()}
    variants = [("candidate", candidate)]
    if len(sys.argv) == 6: variants.insert(0, ("baseline", {"hello": Path(sys.argv[4]).resolve(), "app": Path(sys.argv[5]).resolve()}))
    for _, paths in variants:
        for path in paths.values():
            if not path.is_file() or not os.access(path, os.X_OK): raise ValueError(f"binary is not executable: {path}")
    passes, processes = positive_setting("PASSES", "3"), positive_setting("PROCS", "4")
    connections, duration = positive_setting("CONNS", "16"), positive_setting("DURATION_MS", "5000")
    port = positive_setting("PORT", "18088")
    if port > 65535: raise ValueError("PORT must be in 1..65535")
    runtimes = os.environ.get("RUNTIMES", "kiln node bun deno").split()
    workloads = os.environ.get("WORKLOADS", "hello mixed static-file json-echo-1k json-echo-64k chunked-stream large-upload connection-churn fs-small-reads fs-large-read fs-stat-storm").split()
    if not runtimes or not workloads: raise ValueError("RUNTIMES and WORKLOADS must each contain at least one value")
    result_root = Path(os.environ.get("RESULTS_DIR", ROOT / ".scratch" / f"server-compare-{int(time.time())}-{os.getpid()}"))
    result_root.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="kiln-server-sources-") as temporary:
        sources = Path(temporary)
        for name in ("serve.mjs", "app.mjs"): (sources / name).write_text((HERE / name).read_text().replace("18088", str(port)))
        metadata = {
            "git_commit": subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip(),
            "baseline_source": os.environ.get("BASELINE_SOURCE", ""), "candidate_source": os.environ.get("CANDIDATE_SOURCE", ""),
            "serve_source_sha256": sha256(HERE / "serve.mjs"), "app_source_sha256": sha256(HERE / "app.mjs"),
            "transformed_serve_source_sha256": sha256(sources / "serve.mjs"), "transformed_app_source_sha256": sha256(sources / "app.mjs"),
            "driver_source_sha256": sha256(HERE / "drive.mjs"), "drive_mix_source_sha256": sha256(HERE / "drive-mix.mjs"),
            "drive_pipeline_source_sha256": sha256(HERE / "drive-pipeline.mjs"),
            "variants": {label: {kind: sha256(path) for kind, path in paths.items()} for label, paths in variants},
            "runtime_versions": {runtime: runtime_version(runtime, candidate) for runtime in runtimes if runtime == "kiln" or shutil.which(runtime)},
            "passes": passes, "processes": processes, "connections": connections, "duration_ms": duration,
            "port": port, "runtimes": runtimes, "workloads": workloads,
            "variant_order_by_pass": {
                str(pass_number): {
                    runtime: [label for label, _ in runtime_variant_order(variants, runtime, pass_number)]
                    for runtime in runtimes
                }
                for pass_number in range(1, passes + 1)
            },
        }
        (result_root / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        for pass_number in range(1, passes + 1):
            for workload in workloads:
                driver = HERE / (
                    "drive-mix.mjs" if workload == "mixed" else
                    "drive-pipeline.mjs" if workload == "pipeline" else
                    "drive.mjs"
                )
                for runtime in runtimes:
                    if runtime != "kiln" and shutil.which(runtime) is None:
                        print(f"pass={pass_number} workload={workload:<18} runtime={runtime:<4} skipped (not on PATH)"); continue
                    if not port_free(port): raise RuntimeError(f"port {port} is already occupied")
                    active_variants = runtime_variant_order(variants, runtime, pass_number)
                    for label, variant in active_variants:
                        if not port_free(port): raise RuntimeError(f"port {port} is already occupied before {label}")
                        run_dir = result_root / f"pass-{pass_number}-{label}-{runtime}-{workload}"; run_dir.mkdir()
                        stdout_file, stderr_file = (run_dir / "server.stdout").open("wb"), (run_dir / "server.stderr").open("wb")
                        process = subprocess.Popen(runtime_command(runtime, variant, workload, sources), cwd=ROOT,
                                                   stdout=stdout_file, stderr=stderr_file, start_new_session=True)
                        try:
                            wait_ready(port, process)
                            reports = run_clients(driver, duration, connections, port, workload, processes,
                                                  max(10.0, duration / 1000 + 5.0), output_dir=run_dir)
                            for index, (report, stderr, status) in enumerate(reports, 1):
                                (run_dir / f"client-{index}.json").write_text(json.dumps(report, indent=2) + "\n")
                                if stderr: (run_dir / f"client-{index}.stderr").write_text(stderr)
                                if status != 0 or report.get("errors", 0) or report.get("wrong", 0): raise RuntimeError(f"client validation failed: {report}")
                            aggregate = {"pass": pass_number, "label": label, "runtime": runtime, "workload": workload,
                                         "reports": [row[0] for row in reports], "rps": sum(row[0]["rps"] for row in reports)}
                            (run_dir / "aggregate.json").write_text(json.dumps(aggregate, indent=2) + "\n")
                            print(json.dumps(aggregate, separators=(",", ":")))
                        finally:
                            stop_process(process); stdout_file.close(); stderr_file.close()
                            if not port_free(port): raise RuntimeError(f"owned server left port {port} occupied")
    return 0

if __name__ == "__main__":
    try: raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"compare.py: {error}", file=sys.stderr); raise SystemExit(1)
