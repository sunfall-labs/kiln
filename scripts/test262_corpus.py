"""Shared Test262 checkout, selection, execution, and report policy."""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any

from test262_memory import run_limited, verify_effective_cap

REPOSITORY = "https://github.com/tc39/test262.git"
COMMIT = "771005236e88a909635104e03ba12559688c0172"
OUTCOMES = ("pass", "fail", "unsupported", "skipped")


class CorpusError(RuntimeError):
    pass


def default_checkout() -> pathlib.Path:
    return pathlib.Path(os.environ.get("KILN_TEST262_CHECKOUT", pathlib.Path.home() / ".cache" / "kiln" / f"test262-{COMMIT[:10]}"))


def _file_count(root: pathlib.Path) -> int:
    return sum(1 for p in root.rglob("*") if p.is_file() and p.suffix == ".js")


def validate_checkout(checkout: pathlib.Path, *, minimum_files: int = 1) -> pathlib.Path:
    checkout = checkout.expanduser().resolve(strict=False)
    if not (checkout / ".git").exists():
        raise CorpusError(f"Test262 checkout has no .git directory: {checkout}")
    try:
        head = subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "--verify", "HEAD^{commit}"], text=True, stderr=subprocess.DEVNULL).strip()
    except (OSError, subprocess.CalledProcessError) as exc:
        raise CorpusError(f"cannot resolve Test262 checkout HEAD: {checkout}") from exc
    if head != COMMIT:
        raise CorpusError(f"Test262 checkout is {head}, expected pinned commit {COMMIT}")
    try:
        status = subprocess.check_output(["git", "-C", str(checkout), "status", "--porcelain", "--untracked-files=all"], text=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        raise CorpusError(f"cannot inspect Test262 checkout status: {checkout}") from exc
    if status:
        raise CorpusError(f"Test262 checkout is dirty; refusing to run a modified tree: {checkout}")
    if not (checkout / "harness" / "assert.js").is_file():
        raise CorpusError(f"Test262 harness/assert.js is missing: {checkout}")
    test = checkout / "test"
    if not test.is_dir():
        raise CorpusError(f"Test262 test directory is missing: {checkout}")
    files = _file_count(test)
    if files < minimum_files:
        raise CorpusError(f"Test262 checkout looks empty ({files} JavaScript files under test/, need {minimum_files})")
    return checkout


def fetch_managed(destination: pathlib.Path | None = None, *, minimum_files: int = 1000) -> pathlib.Path:
    requested = pathlib.Path(destination or default_checkout()).expanduser()
    requested = pathlib.Path(os.path.abspath(requested))
    if os.path.lexists(requested) and requested.is_symlink():
        raise CorpusError(f"managed checkout destination is a symlink; refusing to replace it: {requested}")
    destination = requested.resolve(strict=False)
    if destination.exists() and (destination / ".git").exists():
        try:
            return validate_checkout(destination, minimum_files=minimum_files)
        except CorpusError as exc:
            raise CorpusError(f"managed checkout is invalid; refusing to delete it: {exc}") from exc
    if destination.exists():
        raise CorpusError(f"managed checkout path exists but is not a checkout; refusing to delete it: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = pathlib.Path(tempfile.mkdtemp(prefix=f".{destination.name}.staging-", dir=destination.parent))
    try:
        subprocess.run(["git", "init", "--quiet", str(stage)], check=True)
        subprocess.run(["git", "-C", str(stage), "remote", "add", "origin", REPOSITORY], check=True)
        subprocess.run(["git", "-C", str(stage), "fetch", "--quiet", "--depth=1", "origin", COMMIT], check=True)
        subprocess.run(["git", "-C", str(stage), "checkout", "--quiet", "--detach", COMMIT], check=True)
        validate_checkout(stage, minimum_files=minimum_files)
        lock = destination.with_name(destination.name + ".publishing")
        try:
            lock_fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError as exc:
            raise CorpusError(f"another fetch is publishing this checkout: {destination}") from exc
        try:
            if destination.exists():
                raise CorpusError(f"managed checkout appeared during fetch; refusing to replace it: {destination}")
            os.rename(stage, destination)
        finally:
            os.close(lock_fd)
            lock.unlink(missing_ok=True)
        return validate_checkout(destination, minimum_files=minimum_files)
    except Exception:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def resolve_contained_dir(root: pathlib.Path, selected: str, label: str) -> pathlib.Path:
    path = pathlib.PurePosixPath(selected)
    if path.is_absolute() or ".." in path.parts:
        raise CorpusError(f"{label} must be a relative subtree inside test/")
    candidate = (root / pathlib.Path(*path.parts)).resolve()
    try:
        candidate.relative_to(root)
    except ValueError as exc:
        raise CorpusError(f"{label} escapes test/: {selected}") from exc
    if not candidate.is_dir():
        raise CorpusError(f"{label} does not name a directory under test/: {selected}")
    return candidate


def require_nonempty(candidate: pathlib.Path, label: str) -> None:
    if not any(p.is_file() for p in candidate.rglob("*")):
        raise CorpusError(f"{label} names an empty subtree")


def resolve_test_root(checkout: pathlib.Path, selected: str = "") -> tuple[pathlib.Path, str]:
    root = (checkout / "test").resolve()
    if not root.is_dir():
        raise CorpusError(f"Test262 test directory is missing: {root}")
    if not selected or selected == ".":
        candidate, report_path = root, "."
    else:
        candidate = resolve_contained_dir(root, selected, "--path")
        report_path = "/".join(pathlib.PurePosixPath(selected).parts)
    require_nonempty(candidate, f"--path {report_path}")
    return candidate, report_path


def discover_chunks(root: pathlib.Path, existing: list[str] | None = None) -> list[str]:
    if existing:
        chunks = list(existing)
    else:
        chunks = []
        for top in sorted(root.iterdir()):
            if not top.is_dir() or top.name == "harness":
                continue
            children = [p for p in sorted(top.iterdir()) if p.is_dir()]
            chunks.extend(f"{top.name}/{p.name}" for p in children) if children else chunks.append(top.name)
    for chunk in chunks:
        path = resolve_contained_dir(root.resolve(), chunk, "baseline chunk")
        require_nonempty(path, f"baseline chunk {chunk}")
    return chunks


def run_result(kiln: str, root: pathlib.Path, output: pathlib.Path, log: pathlib.Path) -> tuple[dict[str, Any], subprocess.CompletedProcess[str]]:
    verify_effective_cap()
    try:
        output.unlink()
    except FileNotFoundError:
        pass
    except OSError as exc:
        raise CorpusError(f"cannot remove stale result document {output}: {exc}") from exc
    log.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    with log.open("w", encoding="utf-8") as stream:
        completed, exceeded = run_limited([kiln, "--one-shot", "test", str(root), "--out", str(output)], cwd=os.getcwd(), stdout=stream, stderr=subprocess.STDOUT)
    if exceeded:
        raise CorpusError(f"child exceeded memory cap; full log: {log}")
    if not output.is_file():
        raise CorpusError(f"Kiln wrote no result document (exit {completed.returncode}); full log: {log}")
    try:
        report = json.loads(output.read_text(encoding="utf-8"))
        tests = report["tests"]
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise CorpusError(f"invalid Kiln result document; full log: {log}: {exc}") from exc
    if not isinstance(tests, list) or not tests:
        raise CorpusError(f"Kiln result document has no test entries; full log: {log}")
    counts = {key: 0 for key in OUTCOMES}
    paths: set[str] = set()
    for test in tests:
        if not isinstance(test, dict) or not isinstance(test.get("id"), str):
            raise CorpusError("malformed test entry in Kiln result document")
        outcome = test.get("outcome")
        if outcome not in counts:
            raise CorpusError(f"unknown test outcome: {outcome!r}")
        counts[outcome] += 1
        path = test.get("path", test["id"])
        if not isinstance(path, str):
            raise CorpusError("test result path must be a string")
        paths.add(path)
    totals = report.get("totals")
    if not isinstance(totals, dict):
        raise CorpusError("Kiln result document has no totals object")
    expected = {"passed": counts["pass"], "failed": counts["fail"], "unsupported": counts["unsupported"], "skipped": counts["skipped"], "total": len(tests)}
    for key, value in expected.items():
        if not isinstance(totals.get(key), int) or totals[key] != value:
            raise CorpusError(f"Kiln result totals disagree with test entries for {key}: expected {value}, got {totals.get(key)!r}")
    if completed.returncode not in (0, 1) or (completed.returncode == 1 and not (counts["fail"] or counts["unsupported"] or counts["skipped"])):
        raise CorpusError(f"Kiln exited {completed.returncode} without reported test failures; full log: {log}")
    report["_kiln_elapsed_millis"] = max(1, round((time.monotonic() - started) * 1000))
    report["_kiln_paths"] = len(paths)
    report["_kiln_counts"] = counts
    return report, completed


def load_checkout(explicit: str | None) -> pathlib.Path:
    if explicit:
        return validate_checkout(pathlib.Path(explicit), minimum_files=1)
    return fetch_managed()


def command_run(args: argparse.Namespace) -> int:
    checkout = load_checkout(args.checkout)
    root, report_path = resolve_test_root(checkout, args.path)
    output = pathlib.Path(args.raw)
    log = pathlib.Path(args.log)
    report, completed = run_result(args.kiln, root, output, log)
    counts = report.pop("_kiln_counts")
    discovered = report.pop("_kiln_paths")
    elapsed = report.pop("_kiln_elapsed_millis")
    report.update({"corpus": "test262", "path": report_path, "test262": {"repository": REPOSITORY, "commit": COMMIT}, "discovered": discovered, "executed": counts["pass"] + counts["fail"], "passed": counts["pass"], "failed": counts["fail"], "unsupported": counts["unsupported"], "skipped": counts["skipped"], "elapsed_millis": elapsed, "throughput_tests_per_second": round((counts["pass"] + counts["fail"]) / (elapsed / 1000), 3)})
    pathlib.Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    tmp = pathlib.Path(str(args.out) + f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, args.out)
    return completed.returncode


def command_fetch(args: argparse.Namespace) -> int:
    if not args.destination and os.environ.get("KILN_TEST262_CHECKOUT"):
        # An explicitly supplied checkout is user-owned: validate it, never replace or delete it.
        print(validate_checkout(pathlib.Path(os.environ["KILN_TEST262_CHECKOUT"]), minimum_files=1))
    else:
        print(fetch_managed(pathlib.Path(args.destination) if args.destination else None))
    return 0


def command_baseline(args: argparse.Namespace) -> int:
    checkout = load_checkout(args.checkout)
    root, _ = resolve_test_root(checkout)
    baseline_path = pathlib.Path(args.baseline)
    baseline = json.loads(baseline_path.read_text()) if baseline_path.exists() else {"schema": 1, "test262_commit": COMMIT, "chunks": {}}
    if baseline.get("test262_commit") != COMMIT and not args.update:
        raise CorpusError(f"baseline was taken against {baseline.get('test262_commit')}, not {COMMIT}")
    chunks = discover_chunks(root, sorted(baseline.get("chunks", {})) or None)
    if not args.update and not baseline.get("chunks"):
        raise CorpusError("baseline holds no chunks; regenerate it with --update")
    measured, report_chunks, regressions, gains = {}, {}, [], []
    for index, chunk in enumerate(chunks, 1):
        out = pathlib.Path(args.work) / (chunk.replace("/", "_") + ".json")
        log = pathlib.Path(args.logdir) / (chunk.replace("/", "_") + ".log")
        report, _ = run_result(args.kiln, root / chunk, out, log)
        totals = report.get("totals", {})
        total = int(totals.get("total", 0))
        if total <= 0:
            raise CorpusError(f"chunk {chunk} has zero test results; full log: {log}")
        passed, failed = int(totals["passed"]), int(totals["failed"])
        report_chunks[chunk] = {"total": sum(report["_kiln_counts"].values()), **report["_kiln_counts"]}
        measured[chunk] = {"passed": passed, "failed": failed}
        old = baseline.get("chunks", {}).get(chunk)
        mark = ""
        if old:
            if passed < old["passed"]:
                regressions.append(f"{chunk}: {old['passed']} -> {passed} passing"); mark = "  REGRESSED"
            elif passed > old["passed"]:
                gains.append(f"{chunk}: +{passed - old['passed']}"); mark = f"  +{passed - old['passed']}"
        print(f"[{index}/{len(chunks)}] {chunk}  {passed}/{failed}{mark}", flush=True)
    if getattr(args, "report", None):
        report = {
            "schema": 1,
            "status": "failed" if regressions else "passed",
            "source": {"name": "Test262 baseline", "repository": REPOSITORY, "commit": COMMIT},
            "counts": {"total": sum(v["total"] for v in report_chunks.values()), "passed": sum(v["pass"] for v in report_chunks.values()), "failed": sum(v["fail"] for v in report_chunks.values()), "unsupported": sum(v["unsupported"] for v in report_chunks.values()), "skipped": sum(v["skipped"] for v in report_chunks.values())},
            "chunks": report_chunks,
            "expected_chunks": chunks,
            "regressions": regressions,
        }
        report_path = pathlib.Path(args.report)
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if args.update:
        baseline.update({"schema": 1, "test262_commit": COMMIT, "chunks": measured})
        baseline_path.write_text(json.dumps(baseline, indent=2, sort_keys=True) + "\n")
        print(f"\nwrote {baseline_path}: {len(measured)} chunks, {sum(x['passed'] for x in measured.values())} passing")
        return 0
    for gain in gains: print(f"gain     {gain}")
    for regression in regressions: print(f"REGRESS  {regression}", file=sys.stderr)
    if regressions: raise CorpusError(f"{len(regressions)} chunk(s) regressed; accept them with --update if intended")
    print(f"\nno regressions across {len(measured)} chunks, {sum(x['passed'] for x in measured.values())} passing" + (f", {len(gains)} chunk(s) gained" if gains else ""))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    run = sub.add_parser("run")
    run.add_argument("--checkout"); run.add_argument("--path", default=""); run.add_argument("--kiln", required=True); run.add_argument("--raw", required=True); run.add_argument("--out", required=True); run.add_argument("--log", required=True)
    run.set_defaults(function=command_run)
    fetch = sub.add_parser("fetch")
    fetch.add_argument("--destination")
    fetch.set_defaults(function=command_fetch)
    base = sub.add_parser("baseline")
    base.add_argument("--checkout"); base.add_argument("--kiln", required=True); base.add_argument("--baseline", required=True); base.add_argument("--work", required=True); base.add_argument("--logdir", required=True); base.add_argument("--report"); base.add_argument("--update", action="store_true"); base.set_defaults(function=command_baseline)
    args = parser.parse_args()
    try: return args.function(args)
    except (CorpusError, OSError, subprocess.CalledProcessError) as exc:
        print(f"test262 corpus: {exc}", file=sys.stderr); return 1


if __name__ == "__main__":
    raise SystemExit(main())
