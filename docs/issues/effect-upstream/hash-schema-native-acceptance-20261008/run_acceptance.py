#!/usr/bin/env python3
"""Bounded, source-pinned supervisor for Hash + SchemaBinary native acceptance."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import resource
import signal
import subprocess
import sys
import threading
import time


HERE = Path(__file__).resolve().parent
REVIEW = HERE.parents[3]
ROOT = Path("~/Work/kiln/.scratch/effect-sandbox-integration")
WORKSPACE = ROOT / ".scratch/frozen-clock-native/workspace"
SOURCE = WORKSPACE / "source"
RUNNER = ROOT / "scripts/effect-upstream.py"
CLI = REVIEW / ".scratch/final-release/kiln-candidate"
PROVIDER_DIR = REVIEW / ".scratch/final-release/gate-install/lib/kiln"
PROVIDER = PROVIDER_DIR / "kiln-native-provider.json"
PROVIDER_ARCHIVE = PROVIDER_DIR / "libkiln_native_provider_ffi.a"
OUT = HERE / "run"
SPEC = HERE / "prepared-job.json"
RUNNING = HERE / "running.json"
STATUS = HERE / "status.json"
LOG = HERE / "outer.log"

EXPECTED_REVIEW = "554f5c465d7f042dd429c32479c10dffa78143c0"
EXPECTED_PRODUCTION = "624968060241897edfc7d38b40f18aae80a3d2bc"
EXPECTED_EFFECT = "67ba4e46a11ccda0b6761578bfd22c04ae00167d"
EXPECTED_CLI = "c1ee6aa03d420d95bb5c52df29bfe8a770ff4816e802442b996e13ff1da085be"
EXPECTED_FULL_MANIFEST = "f31f0ceebf4a272400e9667d561d119e5f00d451ae3c072c4783375f6deecc9f"
EXPECTED_FULL_ARCHIVE = "fe25b4720dd0741d661b60ceceec3ba490e4941cfd21570ec23b9aecb9c14720"
EXPECTED_HARNESS = "6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27"
EXPECTED_RUNNER = "28211c0c0120db8bcfd0c9380eeb458c4ccbca0a00a0bf4cbb5026ba50af5e61"
EXPECTED_SOURCE_INPUTS = "4728f62e357ecf1a8780021961d9167af3ff0232355f63e2e83c021dcc760ae4"
EXPECTED_PRODUCTION_INPUT_FILES = 651
OUTER_TIMEOUT_S = 5400
RUNNER_TIMEOUT_S = 1200
MEMORY_MB = 8192

PINNED_PATH = ":".join((
    "~/.local/share/mise/installs/rust/1.97.1",
    "~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins",
    "~/.local/share/mise/installs/node/26.8.1/bin",
    "~/.local/share/mise/installs/bun/latest/bin",
    "/usr/bin", "/bin",
))


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    ).stdout.strip()


def production_source_state() -> dict:
    helper_path = REVIEW / ".scratch/final-release/build_final_cli.py"
    spec = importlib.util.spec_from_file_location("kiln_build_final_cli", helper_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load pinned production input digest helper")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    state = module.source_state()
    return {
        "head": state["head"], "tree": state["git_tree"],
        "tracked_clean": state["tracked_worktree_clean"],
        "tracked_status": state["tracked_status"],
        "build_input_file_count": state["build_input_file_count"],
        "build_inputs_sha256": state["build_inputs_sha256"],
    }


def explicit_hashes() -> dict:
    return {
        "cli_sha256": sha(CLI),
        "provider_manifest_sha256": sha(PROVIDER),
        "provider_archive_sha256": sha(PROVIDER_ARCHIVE),
        "hash_entry_sha256": sha(WORKSPACE / "files/Hash/entry.ts"),
        "hash_tsconfig_sha256": sha(WORKSPACE / "files/Hash/tsconfig.json"),
        "hash_test_sha256": sha(SOURCE / "packages/effect/test/Hash.test.ts"),
        "schema_entry_sha256": sha(WORKSPACE / "files/encoding/SchemaBinary/entry.ts"),
        "schema_tsconfig_sha256": sha(WORKSPACE / "files/encoding/SchemaBinary/tsconfig.json"),
        "schema_test_sha256": sha(SOURCE / "packages/effect/test/encoding/SchemaBinary.test.ts"),
        "harness_sha256": sha(WORKSPACE / "harness.mjs"),
        "runner_sha256": sha(RUNNER),
        "reference_sha256": sha(WORKSPACE / "node-vitest-reference.json"),
        "mapping_sha256": sha(WORKSPACE / "node-vitest-case-mapping.json"),
        "mode_policy_sha256": sha(WORKSPACE / "engine-mode-policy.json"),
    }


def expected_cases() -> dict[str, list[dict[str, str]]]:
    oracle = json.loads((WORKSPACE / "node-vitest-reference.json").read_text())
    result = {}
    for name in ("Hash", "encoding/SchemaBinary"):
        rows = oracle["files"][name]["cases"]
        result[name] = [{"id": row["id"], "mode": row["mode"]} for row in rows]
    if len(result["Hash"]) != 25 or len(result["encoding/SchemaBinary"]) != 193:
        raise RuntimeError("official Node/Vitest inventory is not the expected 25 + 193 cases")
    ids = [case["id"] for rows in result.values() for case in rows]
    if len(ids) != len(set(ids)) or len(ids) != 218:
        raise RuntimeError("expected original inventories are not 218 unique IDs")
    return result


def input_state() -> dict:
    source_state = production_source_state()
    if (source_state["head"] != EXPECTED_REVIEW
            or source_state["build_inputs_sha256"] != EXPECTED_SOURCE_INPUTS
            or source_state["build_input_file_count"] != EXPECTED_PRODUCTION_INPUT_FILES):
        raise RuntimeError("review source/build-input identity changed")
    if source_state["tracked_clean"] is not True:
        raise RuntimeError("review tracked source is dirty")
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", EXPECTED_PRODUCTION, EXPECTED_REVIEW],
        cwd=REVIEW, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    if ancestor.returncode != 0:
        raise RuntimeError("frozen production source commit is not an ancestor of review HEAD")
    effect_head = git(SOURCE, "rev-parse", "HEAD")
    effect_status = git(SOURCE, "status", "--porcelain=v1", "--untracked-files=no")
    if effect_head != EXPECTED_EFFECT or effect_status:
        raise RuntimeError("pinned upstream source changed or is dirty")
    hashes = explicit_hashes()
    expected_hashes = {
        "cli_sha256": EXPECTED_CLI,
        "provider_manifest_sha256": EXPECTED_FULL_MANIFEST,
        "provider_archive_sha256": EXPECTED_FULL_ARCHIVE,
        "harness_sha256": EXPECTED_HARNESS,
        "runner_sha256": EXPECTED_RUNNER,
    }
    mismatch = {key: {"expected": value, "actual": hashes[key]}
                for key, value in expected_hashes.items() if hashes[key] != value}
    if mismatch:
        raise RuntimeError("input hash mismatch: " + json.dumps(mismatch, sort_keys=True))
    cases = expected_cases()
    return {
        "review_source": source_state,
        "review_expected_production_commit": EXPECTED_PRODUCTION,
        "effect_source_head": effect_head,
        "effect_source_tracked_clean": not bool(effect_status),
        "effect_source_status": effect_status,
        "files": hashes,
        "case_ids_modes": cases,
    }


def build_command() -> list[str]:
    return [
        sys.executable, str(RUNNER), "--workspace", str(WORKSPACE), "--out", str(OUT),
        "--engine", "native", "--path", "Hash", "--path", "encoding/SchemaBinary",
        "--kiln", str(CLI), "--provider-manifest", str(PROVIDER),
        "--jobs", "1", "--runtime-jobs", "1", "--native-profile", "native-release",
        "--timeout", str(RUNNER_TIMEOUT_S), "--kiln-memory-mb", str(MEMORY_MB),
    ]


def child_environment() -> dict[str, str]:
    env = {
        "PATH": PINNED_PATH, "TZ": "UTC", "KILN_JOBS": "1",
        "RAYON_NUM_THREADS": "1", "TMPDIR": "/var/tmp/kiln",
    }
    for key in ("HOME", "USER", "LOGNAME", "LANG", "LC_ALL"):
        if key in os.environ:
            env[key] = os.environ[key]
    return env


def report_validation() -> dict:
    path = OUT / "report.json"
    if not path.is_file():
        return {"complete": False, "error": "report.json missing"}
    report = json.loads(path.read_text())
    expected = expected_cases()
    selected = report.get("selected_files")
    if selected != ["Hash", "encoding/SchemaBinary"]:
        return {"complete": False, "error": "unexpected selected file inventory", "selected_files": selected}
    rows = {row.get("file"): row for row in report.get("files", [])}
    if set(rows) != set(expected) or len(report.get("files", [])) != 2:
        return {"complete": False, "error": "file rows missing, extra, or duplicated", "files": list(rows)}
    per_file = {}
    all_case_ids = []
    all_complete = True
    for name, wanted in expected.items():
        envelope = rows[name].get("engines", {}).get("native", {})
        inventory = [{"id": row.get("id"), "mode": row.get("mode")} for row in envelope.get("inventory", [])]
        cases = envelope.get("cases", [])
        case_ids = [row.get("id") for row in cases]
        node = envelope.get("node_vitest", {})
        exact_inventory = inventory == wanted
        exact_unique_cases = case_ids == [x["id"] for x in wanted] and len(set(case_ids)) == len(wanted)
        all_case_ids.extend(case_ids)
        summary = envelope.get("summary") or {}
        exact_summary = (summary.get("discovered") == len(wanted)
                         and summary.get("passed") == len(wanted)
                         and summary.get("failed") == 0
                         and summary.get("skipped") == 0)
        complete = (bool(envelope.get("complete")) and exact_inventory
                    and exact_unique_cases and exact_summary and not envelope.get("errors"))
        all_complete &= (complete and node.get("complete") is True
                         and node.get("matches_modes") is True
                         and node.get("matches_outcomes") is True
                         and node.get("mapped_cases") == len(wanted)
                         and not node.get("unmapped_cases")
                         and not node.get("official_unmapped_cases")
                         and not node.get("mapping_errors")
                         and not node.get("mode_mismatches")
                         and not node.get("outcome_mismatches"))
        per_file[name] = {
            "summary": summary, "errors": envelope.get("errors"),
            "complete": envelope.get("complete"), "inventory_exact": exact_inventory,
            "case_ids_exact_unique": exact_unique_cases, "summary_all_pass": exact_summary,
            "node_vitest": node,
        }
    return {
        "complete": all_complete and len(all_case_ids) == 218 and len(set(all_case_ids)) == 218,
        "selected": report.get("selected"), "per_file": per_file,
        "aggregate": report.get("aggregate"), "comparisons": report.get("comparisons"),
        "runner_passed": report.get("passed"), "report_sha256": sha(path),
        "all_passed": report.get("passed") is True,
    }


def descendants(root_pid: int) -> set[int]:
    parents = {}
    for p in Path("/proc").iterdir():
        if not p.name.isdigit():
            continue
        try:
            rest = (p / "stat").read_text().rsplit(")", 1)[1].split()
            parents[int(p.name)] = int(rest[1])
        except (OSError, ValueError, IndexError):
            continue
    result = set()
    changed = True
    while changed:
        old_len = len(result)
        result.update(pid for pid, parent in parents.items() if parent == root_pid or parent in result)
        changed = len(result) != old_len
    return result


def cleanup_owned_groups(root_pid: int) -> list[dict]:
    actions = []
    groups = set()
    for pid in descendants(root_pid):
        try:
            pgid = os.getpgid(pid)
            if pgid != os.getpgrp():
                groups.add(pgid)
        except ProcessLookupError:
            pass
    for pgid in sorted(groups):
        try:
            os.killpg(pgid, signal.SIGTERM)
            actions.append({"pgid": pgid, "signal": "SIGTERM"})
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 2.0
    while time.monotonic() < deadline:
        live = False
        for pgid in groups:
            try:
                os.killpg(pgid, 0)
                live = True
            except ProcessLookupError:
                pass
        if not live:
            break
        time.sleep(0.05)
    for pgid in sorted(groups):
        try:
            os.killpg(pgid, signal.SIGKILL)
            actions.append({"pgid": pgid, "signal": "SIGKILL"})
        except ProcessLookupError:
            pass
    return actions


def pressure_snapshot() -> dict:
    result = {"wall_unix_s": time.time(), "monotonic_s": time.monotonic()}
    try:
        result["loadavg"] = list(os.getloadavg())
    except (AttributeError, OSError):
        result["loadavg"] = None
    for name in ("cpu", "memory", "io"):
        try:
            result[f"psi_{name}"] = (Path("/proc/pressure") / name).read_text().strip()
        except OSError:
            result[f"psi_{name}"] = None
    try:
        result["proc_loadavg"] = Path("/proc/loadavg").read_text().strip()
    except OSError:
        result["proc_loadavg"] = None
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--execute", action="store_true", help="launch only after explicit release")
    parser.add_argument("--preflight-only", action="store_true", help="validate sources and cleanup using a synthetic child")
    args = parser.parse_args()
    if OUT.exists() or RUNNING.exists() or STATUS.exists() or LOG.exists():
        raise SystemExit("refusing to overwrite acceptance outputs")
    pre = input_state()
    env = child_environment()
    cmd = build_command()
    spec = {
        "state": "PREPARED_NOT_LAUNCHED", "command": cmd, "cwd": str(ROOT),
        "output": str(OUT), "outer_timeout_s": OUTER_TIMEOUT_S,
        "runner_timeout_s_each_build_and_run": RUNNER_TIMEOUT_S,
        "original_case_deadline_ms": 5000, "memory_limit_mb_per_kiln_child": MEMORY_MB,
        "environment": {k: env[k] for k in ("PATH", "TZ", "KILN_JOBS", "RAYON_NUM_THREADS", "TMPDIR")},
        "source_preflight": pre,
        "case_ids_modes": pre["case_ids_modes"],
    }
    SPEC.write_text(json.dumps(spec, indent=2, sort_keys=True) + "\n")
    if args.preflight_only:
        # Exercise only this supervisor's process-group cleanup with an owned synthetic sleeper.
        probe = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"], start_new_session=True)
        cleanup = cleanup_owned_groups(os.getpid())
        # The synthetic child is a separate session under the supervisor, so explicitly terminate it.
        try:
            os.killpg(probe.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        probe.wait(timeout=5)
        post = input_state()
        print(json.dumps({"preflight": "passed", "synthetic_cleanup_actions": cleanup,
                          "synthetic_child_reaped": probe.poll() is not None,
                          "source_unchanged": pre == post}, sort_keys=True))
        return 0 if pre == post and probe.poll() is not None else 1
    if not args.execute:
        print(json.dumps({"state": spec["state"], "cases": 218, "prepared_job": str(SPEC)}, sort_keys=True))
        return 0

    OUT.mkdir(parents=True, exist_ok=False)
    start_wall, start_mono = time.time(), time.monotonic()
    before_rusage = resource.getrusage(resource.RUSAGE_CHILDREN)
    snapshots = [pressure_snapshot()]
    stop = threading.Event()

    def sample_pressure():
        while not stop.wait(1.0):
            snapshots.append(pressure_snapshot())

    sampler = threading.Thread(target=sample_pressure, daemon=True)
    run_doc = {"state": "running", "started_wall_unix_s": start_wall,
               "started_monotonic_s": start_mono, "command": cmd, "cwd": str(ROOT),
               "source_pre": pre, "output": str(OUT)}
    RUNNING.write_text(json.dumps(run_doc, indent=2, sort_keys=True) + "\n")
    timed_out = False
    cleanup = []
    with LOG.open("xb") as log:
        child = subprocess.Popen(cmd, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        run_doc.update({"child_pid": child.pid, "process_group_id": child.pid})
        RUNNING.write_text(json.dumps(run_doc, indent=2, sort_keys=True) + "\n")
        sampler.start()
        try:
            code = child.wait(timeout=OUTER_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            timed_out = True
            # Keep the runner alive while discovering any limited-child sessions it spawned.
            cleanup = cleanup_owned_groups(child.pid)
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                code = child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                code = child.wait()
        finally:
            stop.set()
            sampler.join(timeout=3)
            cleanup.extend(cleanup_owned_groups(child.pid))
            try:
                os.killpg(child.pid, signal.SIGTERM)
                cleanup.append({"pgid": child.pid, "signal": "SIGTERM", "reason": "owned runner session cleanup"})
            except ProcessLookupError:
                pass
            if child.poll() is None:
                try:
                    code = child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(child.pid, signal.SIGKILL)
                        cleanup.append({"pgid": child.pid, "signal": "SIGKILL", "reason": "owned runner session reap fallback"})
                    except ProcessLookupError:
                        pass
                    code = child.wait()
    snapshots.append(pressure_snapshot())
    post = input_state()
    after_rusage = resource.getrusage(resource.RUSAGE_CHILDREN)
    validation = report_validation()
    group_absent = False
    try:
        os.killpg(child.pid, 0)
    except ProcessLookupError:
        group_absent = True
    status = {
        "state": "terminal", "exit_code": code, "timed_out": timed_out,
        "started_wall_unix_s": start_wall, "ended_wall_unix_s": time.time(),
        "wall_s": time.monotonic() - start_mono, "child_pid": child.pid,
        "process_group_id": child.pid, "process_group_absent": group_absent,
        "cleanup_actions": cleanup, "log": str(LOG), "log_sha256": sha(LOG),
        "log_bytes": LOG.stat().st_size, "source_pre": pre, "source_post": post,
        "source_inputs_unchanged": pre == post,
        "rusage_children_delta": {"user_cpu_s": after_rusage.ru_utime - before_rusage.ru_utime,
                                   "system_cpu_s": after_rusage.ru_stime - before_rusage.ru_stime,
                                   "max_rss_kib_highwater": after_rusage.ru_maxrss},
        "host_pressure": {"interval_s": 1.0, "samples": snapshots},
        "report_validation": validation,
    }
    STATUS.write_text(json.dumps(status, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"state": "terminal", "exit_code": code, "timed_out": timed_out,
                      "source_inputs_unchanged": pre == post, "process_group_absent": group_absent,
                      "report_validation": validation}, sort_keys=True))
    return 0 if (code == 0 and not timed_out and pre == post and group_absent
                 and validation.get("complete") and validation.get("all_passed")) else 1


if __name__ == "__main__":
    raise SystemExit(main())
