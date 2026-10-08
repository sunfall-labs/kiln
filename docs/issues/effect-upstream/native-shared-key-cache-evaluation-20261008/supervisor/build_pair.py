#!/usr/bin/env python3
"""Build the matched production624/candidate workload pair serially; never run timings here."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

BENCH = Path(__file__).resolve().parent
REVIEW = BENCH.parents[2]
CANDIDATE_REPO = REVIEW / ".scratch/source-candidates/native-intern-cache"
CANDIDATE_BUILD = CANDIDATE_REPO / ".scratch/native-intern-cache/candidate-release-caf6e0f4-retry1"
PARENT_CLI = REVIEW / ".scratch/final-release/kiln-candidate"
PARENT_CLI_RECEIPT = REVIEW / ".scratch/final-release/receipt.json"
CANDIDATE_CLI = CANDIDATE_BUILD / "kiln-candidate"
CANDIDATE_CLI_RECEIPT = CANDIDATE_BUILD / "receipt.json"
PROVIDER_DIR = REVIEW / ".scratch/final-release/gate-install/lib/kiln"
MANIFEST = PROVIDER_DIR / "kiln-native-provider.json"
ARCHIVE = PROVIDER_DIR / "libkiln_native_provider_ffi.a"
PROJECT = BENCH / "project-boundary"
WORKLOAD = PROJECT / "workload.ts"
HARNESS = PROJECT / "harness.mjs"
BOUNDARY_FILE = PROJECT / "pnpm-workspace.yaml"
OUT = BENCH / "output/build-pair-retry1"
AUDITOR = BENCH / "audit_generated_write.py"
PARENT_SOURCE = "624968060241897edfc7d38b40f18aae80a3d2bc"
CANDIDATE_SOURCE = "caf6e0f4312487795bf89b9bb246db747f3480e4"
PARENT_CLI_SHA = "c1ee6aa03d420d95bb5c52df29bfe8a770ff4816e802442b996e13ff1da085be"
CANDIDATE_CLI_SHA = "a843877cc4bc1d6ceffa582d59f5c178e220d8781bd8a5302deb1fbbf229de1a"
MANIFEST_SHA = "f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f"
ARCHIVE_SHA = "fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720"
WORKLOAD_SHA = "eaeead3d97e32e0508321fe18e4713223fbe37a811bac15fa97a7b7114579d54"
HARNESS_SHA = "b041101125e8f8e789321983bd5ec1579c3a8a8d63abf584adaae43afdcaf827"
BOUNDARY_SHA = "b379483d4293500af8e9d3e8d83b4b5a2ad1e844911cd7be7f7e274e4bb80666"
TIMEOUT_S = 2400
MIN_FREE = 1 * 1024**3
PINNED_PATH = ":".join([
    "~/.local/share/mise/installs/rust/1.97.1",
    "~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins",
    "/usr/bin", "/bin",
])
SHARED = Path("~/Work/kiln/.scratch/effect-sandbox-integration")


def sha(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def git_output(repo: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(repo), *args], check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.strip()


def file_at_commit(repo: Path, commit: str, rel: str) -> bytes:
    return subprocess.run(["git", "-C", str(repo), "show", f"{commit}:{rel}"], check=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout


def source_callpath_receipt() -> dict:
    changed = set(git_output(CANDIDATE_REPO, "diff", "--name-only",
                             PARENT_SOURCE, CANDIDATE_SOURCE).splitlines())
    expected_changes = {
        "crates/native/kiln-backend-c/src/runtime/heap.c",
        "crates/native/kiln-backend-c/src/tests/runtime.rs",
    }
    if changed != expected_changes:
        raise RuntimeError(f"unexpected source delta between CLI sources: {sorted(changed)}")
    paths = [
        "crates/native/kiln-backend-c/src/runtime/boxed.c",
        "crates/native/kiln-backend-c/src/runtime/symbols.c",
        "crates/native/kiln-backend-c/src/runtime/write.c",
        "crates/native/kiln-backend-c/src/runtime/properties.c",
        "crates/native/kiln-backend-c/src/emit_graph/ops/properties.rs",
    ]
    sources = {}
    for rel in paths:
        old = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE, rel)
        new = file_at_commit(CANDIDATE_REPO, CANDIDATE_SOURCE, rel)
        if old != new:
            raise RuntimeError(f"runtime callpath source unexpectedly differs: {rel}")
        sources[rel] = {"sha256": sha_bytes(old), "equal_between_sources": True}
    write = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                           "crates/native/kiln-backend-c/src/runtime/write.c").decode()
    props = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                           "crates/native/kiln-backend-c/src/runtime/properties.c").decode()
    symbols = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                             "crates/native/kiln-backend-c/src/runtime/symbols.c").decode()
    emitter = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                             "crates/native/kiln-backend-c/src/emit_graph/ops/properties.rs").decode()
    boxed = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                           "crates/native/kiln-backend-c/src/runtime/boxed.c").decode()
    checks = {
        "emitter_calls_dynamic_write_entry": "kiln_c99_val_write_element(host, v{o}, {vk}, v{val}" in emitter,
        "write_path_calls_put_mode_with_property_ikey": "kiln_c99_put_mode_key(host, target, (const kiln_string *)text.ptr,\n                                     kiln_c99_property_ikey(key)" in write,
        "non_symbol_property_ikey_is_null": "key.tag == KILN_VALUE_SYMBOL ? kiln_c99_symbol_ikey" in symbols and ": NULL;" in symbols,
        "put_mode_interns_null_ikey_before_shape_lookup": "if (ikey == NULL) {\n        ikey = kiln_c99_intern_key(key);" in props,
        "boxed_computed_write_enters_set_computed_mode": "kiln_c99_set_computed_mode(host, target, key, value, strict, depth, thrown)" in boxed,
    }
    if not all(checks.values()):
        raise RuntimeError(f"runtime callpath source proof failed: {checks}")
    heap_old = file_at_commit(CANDIDATE_REPO, PARENT_SOURCE,
                              "crates/native/kiln-backend-c/src/runtime/heap.c")
    heap_new = file_at_commit(CANDIDATE_REPO, CANDIDATE_SOURCE,
                              "crates/native/kiln-backend-c/src/runtime/heap.c")
    if b"kiln_c99_shared_key_cache_find" in heap_old or b"kiln_c99_shared_key_cache_find" not in heap_new:
        raise RuntimeError("shared cache source delta did not match the expected runtime variant")
    return {
        "baseline_commit": PARENT_SOURCE,
        "candidate_commit": CANDIDATE_SOURCE,
        "changed_tracked_files": sorted(changed),
        "unchanged_runtime_callpath_sources": sources,
        "callpath_checks": checks,
        "heap_c": {"baseline_sha256": sha_bytes(heap_old), "candidate_sha256": sha_bytes(heap_new)},
    }


def sha_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def receipt_cli(path: Path, receipt_path: Path, expected_source: str) -> tuple[str, dict]:
    if not path.is_file() or not receipt_path.is_file():
        raise RuntimeError(f"frozen CLI or build receipt missing: {path}, {receipt_path}")
    receipt = json.loads(receipt_path.read_text())
    if receipt.get("build_artifact_accepted") is not True:
        raise RuntimeError(f"CLI release artifact not accepted: {receipt_path}")
    source_before = receipt.get("source_before", {})
    if source_before.get("head") != expected_source or source_before.get("tracked_worktree_clean") is not True:
        raise RuntimeError(f"CLI source provenance mismatch in {receipt_path}")
    binary_sha = sha(path)
    staged = receipt.get("staged_cli")
    if staged and Path(staged).resolve() != path.resolve():
        # Candidate receipt may point to a staged copy elsewhere; its own hash still pins it.
        if receipt.get("staged_cli_sha256") != binary_sha:
            raise RuntimeError(f"staged CLI path/hash mismatch in {receipt_path}")
    expected_sha = receipt.get("staged_cli_sha256") or receipt.get("cli_sha256")
    if expected_sha and binary_sha != expected_sha:
        raise RuntimeError(f"CLI bytes differ from receipt: {receipt_path}")
    return binary_sha, receipt


def hash_inputs() -> dict:
    inputs = {
        "parent_cli": PARENT_CLI, "candidate_cli": CANDIDATE_CLI,
        "parent_cli_receipt": PARENT_CLI_RECEIPT, "candidate_cli_receipt": CANDIDATE_CLI_RECEIPT,
        "provider_manifest": MANIFEST, "provider_archive": ARCHIVE,
        "workload": WORKLOAD, "harness": HARNESS, "project_boundary": BOUNDARY_FILE,
    }
    return {name: sha(path) for name, path in inputs.items()}


def disk_free(path: Path) -> int:
    stat = os.statvfs(path)
    return stat.f_bavail * stat.f_frsize


def host_state() -> dict:
    state = {"unix_s": time.time()}
    try:
        state["loadavg"] = Path("/proc/loadavg").read_text().strip()
    except OSError:
        pass
    return state


def write_json(path: Path, value: dict) -> None:
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, indent=2) + "\n")
    temp.replace(path)


def terminate_group(pgid: int, actions: list[dict], reason: str) -> None:
    try:
        os.killpg(pgid, signal.SIGTERM)
        actions.append({"signal": "SIGTERM", "pgid": pgid, "reason": reason, "unix_s": time.time()})
    except ProcessLookupError:
        actions.append({"signal": "none", "pgid": pgid, "reason": reason, "detail": "group absent"})
        return
    end = time.monotonic() + 5
    while time.monotonic() < end:
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


def run_one(name: str, cli: Path, binary: Path, inputs: dict, env: dict) -> dict:
    log = OUT / f"{name}.build.log"
    status_path = OUT / f"{name}.build.status.json"
    command = [str(cli), "--one-shot", "build", str(WORKLOAD), "--profile", "native-release",
               "--output", str(binary)]
    started = time.time()
    pgid = None
    child = None
    actions: list[dict] = []
    timed_out = False
    exit_code = None
    error = None
    free_min = disk_free(OUT)
    running = {"status": "prelaunch", "variant": name, "command": command,
               "cwd": str(PROJECT), "inputs_sha256_before": inputs,
               "environment": {k: env.get(k) for k in (
                   "PATH", "RUSTUP_TOOLCHAIN", "MBX_CACHE_DIR", "MBX_TARGET_ROOT", "MBX_GC_AUTO",
                   "MBX_SCHEDULER_CPUS", "CARGO_BUILD_JOBS", "KILN_JOBS", "RAYON_NUM_THREADS",
                   "TMPDIR", "TZ", "KILN_NATIVE_PROVIDER_MANIFEST")},
               "host_before": host_state(), "started_unix_s": started,
               "timeout_seconds": TIMEOUT_S, "disk_minimum_bytes": MIN_FREE}
    write_json(OUT / f"{name}.build.running.json", running)
    try:
        with log.open("wb") as stream:
            child = subprocess.Popen(command, cwd=PROJECT, env=env, stdout=stream,
                                     stderr=subprocess.STDOUT, start_new_session=True,
                                     preexec_fn=apply_address_cap)
            pgid = os.getpgid(child.pid)
            running.update({"status": "running", "pid": child.pid, "pgid": pgid})
            write_json(OUT / f"{name}.build.running.json", running)
            deadline = time.monotonic() + TIMEOUT_S
            while True:
                free_min = min(free_min, disk_free(OUT))
                if free_min < MIN_FREE:
                    terminate_group(pgid, actions, "free disk below 1 GiB")
                    exit_code = child.wait()
                    error = "disk guard triggered"
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    timed_out = True
                    terminate_group(pgid, actions, "build timeout")
                    exit_code = child.wait()
                    break
                try:
                    exit_code = child.wait(timeout=min(1.0, remaining))
                    break
                except subprocess.TimeoutExpired:
                    continue
    except BaseException as exc:
        error = f"{type(exc).__name__}: {exc}"
        if child is not None and pgid is not None:
            terminate_group(pgid, actions, "supervisor exception")
            exit_code = child.wait()
    finally:
        if child is not None and pgid is not None:
            terminate_group(pgid, actions, "final group cleanup")
            if child.poll() is None:
                exit_code = child.wait()

    after_inputs = hash_inputs()
    binary_sha = sha(binary) if binary.is_file() else None
    result = {
        **running, "status": "terminal", "exit_code": exit_code, "timed_out": timed_out,
        "error": error, "ended_unix_s": time.time(), "wall_seconds": time.time() - started,
        "inputs_sha256_after": after_inputs, "inputs_unchanged": inputs == after_inputs,
        "host_after": host_state(), "free_bytes_minimum_observed": free_min,
        "process_group_cleanup": actions, "process_group_absent": pgid is None or not process_group_exists(pgid),
        "leader_wait_reaped": child is not None and child.poll() is not None,
        "binary_path": str(binary), "binary_sha256": binary_sha,
        "binary_bytes": binary.stat().st_size if binary.is_file() else None,
        "build_log": str(log), "build_log_sha256": sha(log) if log.is_file() else None,
    }
    write_json(status_path, result)
    write_json(OUT / f"{name}.build.running.json", result)
    if (exit_code != 0 or timed_out or error or not result["inputs_unchanged"] or not binary_sha
            or not result["leader_wait_reaped"] or not result["process_group_absent"]):
        raise RuntimeError(f"{name} build failed; see {status_path}")
    return result


def process_group_exists(pgid: int) -> bool:
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def apply_address_cap() -> None:
    import resource
    cap = 8 * 1024**3
    resource.setrlimit(resource.RLIMIT_AS, (cap, cap))


def run_audit(label: str) -> dict:
    spec = importlib.util.spec_from_file_location("audit_generated_write", AUDITOR)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load generated-C auditor")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    result = mod.extract(PROJECT / ".kiln/cache")
    result["label"] = label
    result["cache_root"] = str((PROJECT / ".kiln/cache").resolve())
    result["audit_py_sha256"] = sha(AUDITOR)
    audit_path = OUT / f"{label}.generated-c-audit.json"
    write_json(audit_path, result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--start", action="store_true", help="perform serial builds; requires explicit lane release")
    args = parser.parse_args()
    if not args.start:
        parser.error("no build starts without --start")
    if OUT.exists() and any(OUT.iterdir()):
        raise SystemExit(f"refusing to overwrite build evidence: {OUT}")
    OUT.mkdir(parents=True, exist_ok=True)
    if not (PROJECT / "pnpm-workspace.yaml").is_file():
        raise SystemExit("isolated project boundary is missing")

    parent_cli_sha, parent_receipt = receipt_cli(PARENT_CLI, PARENT_CLI_RECEIPT, PARENT_SOURCE)
    candidate_cli_sha, candidate_receipt = receipt_cli(CANDIDATE_CLI, CANDIDATE_CLI_RECEIPT, CANDIDATE_SOURCE)
    if parent_cli_sha != PARENT_CLI_SHA:
        raise SystemExit("production CLI digest differs from frozen receipt")
    if candidate_cli_sha != CANDIDATE_CLI_SHA:
        raise SystemExit("candidate CLI digest differs from accepted retry1 receipt")
    if candidate_receipt.get("process_group_absent") is not True or candidate_receipt.get("leader_wait_reaped") is not True:
        raise SystemExit("candidate CLI receipt lacks clean process-group completion")
    if sha(MANIFEST) != MANIFEST_SHA or sha(ARCHIVE) != ARCHIVE_SHA:
        raise SystemExit("Full provider manifest/archive digest mismatch")
    fixed = {"workload": sha(WORKLOAD), "harness": sha(HARNESS), "pnpm_boundary": sha(BOUNDARY_FILE)}
    expected = {"workload": WORKLOAD_SHA, "harness": HARNESS_SHA, "pnpm_boundary": BOUNDARY_SHA}
    if fixed != expected:
        raise SystemExit(f"benchmark project inputs differ: expected {expected}, got {fixed}")
    if disk_free(OUT) < MIN_FREE + 64 * 1024**2:
        raise SystemExit("refusing build: need at least 1 GiB plus launch margin free")
    callpath = source_callpath_receipt()

    env = os.environ.copy()
    env.update({
        "PATH": PINNED_PATH, "RUSTUP_TOOLCHAIN": "1.97.1",
        "MBX_CACHE_DIR": str(SHARED / ".mbx-cache"),
        "MBX_TARGET_ROOT": str(SHARED / ".mbx-targets"),
        "MBX_GC_AUTO": "false", "MBX_SCHEDULER_CPUS": "6",
        "CARGO_BUILD_JOBS": "1", "KILN_JOBS": "1", "RAYON_NUM_THREADS": "1",
        "TMPDIR": "/var/tmp/kiln", "TZ": "UTC",
        "KILN_NATIVE_PROVIDER_MANIFEST": str(MANIFEST),
    })
    for key in (
        "KILN_NATIVE_NUMBER_PROVIDER_MANIFEST", "KILN_NATIVE_GC_STRESS", "KILN_GC_STRESS",
        "KILN_REALMS", "KILN_WORKERS", "KILN_BIN", "KILN_TYPE_PROFILE_FILE",
        "KILN_DEBUG_MODULE_DEMANDS", "KILN_NATIVE_TRACE_EMIT", "KILN_HTTP_TRACE",
        "KILN_HEAP_PROFILE", "KILN_HEAP_PROFILE_OBJECTS", "KILN_PARALLEL_TRACE",
    ):
        env.pop(key, None)
    if not Path(env["TMPDIR"]).is_dir():
        raise SystemExit("prepared TMPDIR is unavailable")
    inputs_before = hash_inputs()
    outputs = {"production624": OUT / "production624.bin", "shared-cache-candidate": OUT / "candidate.bin"}
    if any(path.exists() for path in outputs.values()):
        raise SystemExit("refusing to overwrite staged binary output")
    running = {
        "status": "prelaunch", "started_unix_s": time.time(), "source_inputs": fixed,
        "cli_sha256": {"production624": parent_cli_sha, "candidate": candidate_cli_sha},
        "cli_receipt_sha256": {"production624": sha(PARENT_CLI_RECEIPT), "candidate": sha(CANDIDATE_CLI_RECEIPT)},
        "provider": {"manifest_sha256": sha(MANIFEST), "archive_sha256": sha(ARCHIVE),
                     "manifest_path": str(MANIFEST), "archive_path": str(ARCHIVE)},
        "runtime_callpath_source_audit": callpath,
        "project_boundary": str(PROJECT), "project_cache_shared_between_serial_builds": True,
        "build_order": ["production624", "shared-cache-candidate"],
        "builds_are_not_timing_measurements": True,
        "environment": {key: env.get(key) for key in (
            "PATH", "RUSTUP_TOOLCHAIN", "MBX_CACHE_DIR", "MBX_TARGET_ROOT", "MBX_GC_AUTO",
            "MBX_SCHEDULER_CPUS", "CARGO_BUILD_JOBS", "KILN_JOBS", "RAYON_NUM_THREADS",
            "TMPDIR", "TZ", "KILN_NATIVE_PROVIDER_MANIFEST")},
        "inputs_sha256_before": inputs_before, "host_before": host_state(),
    }
    write_json(OUT / "build.running.json", running)
    results = {}
    try:
        for variant, cli in (("production624", PARENT_CLI), ("shared-cache-candidate", CANDIDATE_CLI)):
            inputs = hash_inputs()
            results[variant] = run_one(variant, cli, outputs[variant], inputs, env)
            results[variant]["generated_c_audit"] = run_audit(variant)
            write_json(OUT / "build.running.json", {**running, "status": "running",
                                                       "last_terminal_variant": variant,
                                                       "results": results})
    except BaseException as error:
        status = {**running, "status": "terminal", "overall": "failed",
                  "error": f"{type(error).__name__}: {error}", "results": results,
                  "inputs_sha256_after": hash_inputs(), "host_after": host_state(),
                  "ended_unix_s": time.time()}
        write_json(OUT / "build.status.json", status)
        write_json(OUT / "build.running.json", status)
        raise

    after = hash_inputs()
    audits = [results[v]["generated_c_audit"]["workload_module"]["module_c_sha256"]
              for v in ("production624", "shared-cache-candidate")]
    status = {
        **running, "status": "terminal", "overall": "pass" if inputs_before == after and audits[0] == audits[1] else "failed",
        "results": results, "module_c_payloads_identical": audits[0] == audits[1],
        "application_object_reuse_claim": "none; report cache keys/hits separately if available",
        "inputs_sha256_after": after, "inputs_unchanged": inputs_before == after,
        "host_after": host_state(), "ended_unix_s": time.time(),
        "timing_performed": False,
    }
    write_json(OUT / "build.status.json", status)
    write_json(OUT / "build.running.json", status)
    return 0 if status["overall"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
