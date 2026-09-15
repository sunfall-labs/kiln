"""Validated result envelopes used by the aggregate gate.

The individual runners keep their own comparison rules.  This module only makes their
observable run status and evidence uniform so the gate cannot mistake an empty or malformed
run for success.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
from typing import Any


class ResultError(ValueError):
    pass


def code_snapshot() -> str:
    """Hash tracked working-tree contents, excluding generated/untracked evidence."""
    try:
        names = subprocess.check_output(["git", "ls-files", "-z"], text=False).split(b"\0")
        digest = hashlib.sha256()
        for raw in sorted(name for name in names if name):
            path = Path(raw.decode())
            try:
                info = path.lstat()
            except FileNotFoundError:
                digest.update(len(raw).to_bytes(8, "big") + raw + b"DELETED")
                continue
            mode = info.st_mode
            if path.is_symlink():
                payload = b"SYMLINK" + os.readlink(path).encode()
            elif path.is_file():
                payload = b"FILE" + path.read_bytes()
            else:
                payload = b"OTHER"
            digest.update(len(raw).to_bytes(8, "big") + raw + mode.to_bytes(4, "big") + len(payload).to_bytes(8, "big") + payload)
        return digest.hexdigest()
    except (OSError, subprocess.CalledProcessError, UnicodeDecodeError) as exc:
        raise ResultError(f"cannot compute tracked code snapshot: {exc}") from exc


def binary_snapshot() -> str | None:
    path = os.environ.get("KILN_BIN")
    if not path:
        return None
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        return digest.hexdigest()
    except OSError:
        return None


def _nonnegative(value: Any, name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ResultError(f"{name} must be a non-negative integer")
    return value


def validate(doc: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(doc, dict) or doc.get("schema") != 1:
        raise ResultError("result envelope must declare schema 1")
    verdict = doc.get("verdict", doc.get("status"))
    if verdict not in {"passed", "failed", "infrastructure", "skipped"}:
        raise ResultError("result envelope has invalid verdict")
    source = doc.get("source")
    if not isinstance(source, dict) or not isinstance(source.get("kind"), str) or not source["kind"]:
        raise ResultError("result envelope has no source kind")
    if not isinstance(source.get("code_snapshot_sha256"), str) or not source["code_snapshot_sha256"]:
        raise ResultError("result envelope has no code snapshot identity")
    if source.get("kind") in {"process", "workspace", "conformance", "test262"} and doc.get("verdict") == "passed" and (not isinstance(source.get("binary_sha256"), str) or not source["binary_sha256"]):
        raise ResultError("passed executable step has no binary identity")
    evidence = doc.get("evidence")
    if not isinstance(evidence, dict) or not isinstance(evidence.get("log"), str) or not evidence["log"]:
        raise ResultError("result envelope has no log evidence")
    counts = doc.get("outcomes")
    if counts is None and doc.get("measurement") == "process":
        return doc
    if not isinstance(counts, dict):
        raise ResultError("result envelope has no outcomes")
    total = _nonnegative(counts.get("total"), "counts.total")
    passed = _nonnegative(counts.get("passed"), "counts.passed")
    failed = _nonnegative(counts.get("failed"), "counts.failed")
    if passed + failed > total:
        raise ResultError("counts passed + failed exceeds total")
    categories = []
    for key, value in counts.items():
        if key == "total":
            continue
        categories.append(_nonnegative(value, f"outcomes.{key}"))
    if sum(categories) != total:
        raise ResultError("outcome counts do not sum to total")
    if total == 0 and verdict != "infrastructure":
        raise ResultError("result envelope has no measured cases")
    return doc


def _envelope(source: str, kind: str, log: str, status: int, total: int, passed: int, failed: int, *, verdict: str | None = None, **extra: Any) -> dict[str, Any]:
    snapshot = os.environ.get("KILN_CODE_SNAPSHOT_SHA256") or code_snapshot()
    verdict = verdict or ("passed" if status == 0 and failed == 0 else "failed")
    breakdown = extra.pop("outcome_extra", {})
    outcomes = {"total": total, "passed": passed, "failed": failed, **breakdown}
    result = {"schema": 1, "verdict": verdict, "source": {"kind": kind, "name": source, "code_snapshot_sha256": snapshot, "binary_sha256": binary_snapshot(), "corpus_pin": extra.pop("corpus_pin", None)}, "outcomes": outcomes, "evidence": {"log": str(Path(log).resolve())}, **extra}
    return validate(result)


def write_atomic(path: str, document: dict[str, Any]) -> None:
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_name(f".{target.name}.{__import__('os').getpid()}.tmp")
    temporary.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    temporary.replace(target)


def workspace(log: str, status: int) -> dict[str, Any]:
    text = Path(log).read_text(encoding="utf-8")
    rows = re.findall(r"^test result:\s+(?:ok|FAILED)\.\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored;", text, re.M)
    if not rows:
        raise ResultError("workspace test log contains no test result summaries")
    passed = sum(int(p) for p, _, _ in rows)
    failed = sum(int(f) for _, f, _ in rows)
    ignored = sum(int(i) for _, _, i in rows)
    minimum = int(os.environ.get("KILN_GATE_MIN_PASSED", "500"))
    if passed < minimum:
        raise ResultError(f"workspace tests passed {passed}, below the floor of {minimum}")
    return _envelope("workspace tests", "workspace", log, status, passed + failed + ignored, passed, failed, suites=len(rows), outcome_extra={"ignored": ignored})


def conformance(log: str, status: int, document: str | None = None) -> dict[str, Any]:
    if not document:
        raise ResultError("conformance requires its schema-1 result document")
    try:
        report = json.loads(Path(document).read_text(encoding="utf-8"))
        tests = report["tests"]
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise ResultError(f"invalid conformance result document: {exc}") from exc
    if not isinstance(tests, list) or not tests:
        raise ResultError("conformance result document has no test entries")
    counts = {"pass": 0, "fail": 0, "unsupported": 0, "skipped": 0, "not-yet-runnable": 0}
    for test in tests:
        if not isinstance(test, dict) or test.get("outcome") not in counts:
            raise ResultError("conformance result document has an invalid test outcome")
        counts[test["outcome"]] += 1
    totals = report.get("totals")
    if totals is not None:
        expected = {"total": len(tests), "passed": counts["pass"], "failed": counts["fail"], "unsupported": counts["unsupported"], "skipped": counts["skipped"]}
        if not isinstance(totals, dict) or any(totals.get(key) != value for key, value in expected.items() if key in totals):
            raise ResultError("conformance totals disagree with test outcomes")
    return _envelope("conformance", "conformance", log, status, len(tests), counts["pass"], counts["fail"], outcome_extra={"unsupported": counts["unsupported"], "skipped": counts["skipped"], "not-yet-runnable": counts["not-yet-runnable"]}, result_document=str(Path(document).resolve()))


def test262(log: str, status: int) -> dict[str, Any]:
    raise ResultError("Test262 requires its aggregate result document")


def process(log: str, status: int, name: str) -> dict[str, Any]:
    snapshot = os.environ.get("KILN_CODE_SNAPSHOT_SHA256") or code_snapshot()
    result = {"schema": 1, "verdict": "passed" if status == 0 else "failed", "measurement": "process", "source": {"kind": "process", "name": name, "code_snapshot_sha256": snapshot, "binary_sha256": binary_snapshot()}, "evidence": {"log": str(Path(log).resolve())}}
    return validate(result)


def aggregate(directory: str, expected: list[str], output: str) -> dict[str, Any]:
    documents = []
    for path in sorted(Path(directory).glob("*.envelope.json")):
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
            documents.append(validate(document))
        except (OSError, ValueError) as exc:
            raise ResultError(f"invalid step envelope {path}: {exc}") from exc
    names = [doc.get("source", {}).get("name") for doc in documents]
    if sorted(names) != sorted(expected) or len(names) != len(set(names)):
        raise ResultError(f"gate step set mismatch: expected {expected}, observed {names}")
    snapshots = {doc.get("source", {}).get("code_snapshot_sha256") for doc in documents}
    current = code_snapshot()
    if len(snapshots) != 1 or snapshots != {current}:
        raise ResultError("gate steps were produced from mixed code snapshots")
    binaries = {doc.get("source", {}).get("binary_sha256") for doc in documents if doc.get("source", {}).get("binary_sha256")}
    current_binary = binary_snapshot()
    if len(binaries) > 1 or (binaries and current_binary not in binaries):
        raise ResultError("gate steps were produced with mixed binaries")
    result = {"schema": 1, "verdict": "passed" if all(doc["verdict"] == "passed" for doc in documents) else "failed", "source": {"kind": "gate", "name": "gate aggregate", "code_snapshot_sha256": snapshots.pop(), "binary_sha256": next(iter(binaries), None)}, "evidence": {"steps": names}, "steps": documents, "failed_steps": [name for name, doc in zip(names, documents) if doc["verdict"] != "passed"]}
    write_atomic(output, result)
    return result


def test262_document(log: str, status: int, document: str) -> dict[str, Any]:
    try:
        report = json.loads(Path(document).read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ResultError(f"invalid Test262 result document: {exc}") from exc
    if not isinstance(report, dict) or not isinstance(report.get("chunks"), dict) or not isinstance(report.get("expected_chunks"), list):
        raise ResultError("Test262 result document has no chunk measurements")
    if set(report["chunks"]) != set(report["expected_chunks"]):
        raise ResultError("Test262 result document is incomplete")
    if report.get("status") not in {"passed", "failed"}:
        raise ResultError("Test262 result document has invalid status")
    if status != 0 and report["status"] == "passed":
        raise ResultError(f"Test262 baseline exited {status} despite a passing report")
    counts = report.get("counts")
    if not isinstance(counts, dict):
        raise ResultError("Test262 result document has no counts")
    summed = {"total": 0, "passed": 0, "failed": 0, "unsupported": 0, "skipped": 0}
    for chunk, values in report["chunks"].items():
        if not isinstance(values, dict):
            raise ResultError(f"Test262 chunk {chunk} is malformed")
        for key in summed:
            summed[key] += _nonnegative(values.get(key), f"chunk {chunk}.{key}")
    if any(counts.get(key) != value for key, value in summed.items()):
        raise ResultError("Test262 aggregate counts disagree with chunks")
    if report["status"] == "passed" and report.get("regressions"):
        raise ResultError("Test262 passing report lists regressions")
    doc = _envelope("Test262 baseline", "test262", log, status, counts.get("total"), counts.get("passed"), counts.get("failed"), verdict=report["status"] if status == 0 else "failed", outcome_extra={"unsupported": counts.get("unsupported", 0), "skipped": counts.get("skipped", 0)}, chunks=len(report["chunks"]), result_document=str(Path(document).resolve()), regressions=report.get("regressions", []), corpus_pin=report.get("source", {}).get("commit"))
    return doc


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("kind", choices=("workspace", "conformance", "test262", "process", "aggregate"))
    parser.add_argument("--log")
    parser.add_argument("--status", type=int, default=0)
    parser.add_argument("--out", required=True)
    parser.add_argument("--document")
    parser.add_argument("--name", default="gate step")
    parser.add_argument("--directory")
    parser.add_argument("--expected", action="append", default=[])
    args = parser.parse_args()
    try:
        if args.kind == "aggregate":
            doc = aggregate(args.directory, args.expected, args.out)
            print(f"result: {doc['verdict']} aggregate ({len(args.expected)} steps)")
            return 0 if doc["verdict"] == "passed" else 1
        if not args.log:
            raise ResultError("--log is required for a step result")
        fn = {"workspace": workspace, "conformance": conformance, "test262": test262_document if args.document else test262, "process": process}[args.kind]
        if args.kind in {"conformance", "test262"} and args.document:
            doc = fn(args.log, args.status, args.document)
        elif args.kind == "process":
            doc = fn(args.log, args.status, args.name)
        else:
            doc = fn(args.log, args.status)
        if args.name != "gate step":
            doc["source"]["name"] = args.name
        write_atomic(args.out, doc)
        observed = "process" if doc.get("measurement") == "process" else f"{doc['outcomes']['passed']}/{doc['outcomes']['total']} passed"
        print(f"result: {doc['verdict']} {doc['source']['name']} {observed}; evidence {doc['evidence']['log']}")
        return 0 if doc["verdict"] == "passed" else 1
    except (OSError, ResultError) as exc:
        try:
            snapshot = os.environ.get("KILN_CODE_SNAPSHOT_SHA256") or code_snapshot()
        except ResultError:
            snapshot = "unavailable"
        evidence_path = args.log or args.directory or "."
        step_name = args.name if args.name != "gate step" else args.kind
        failure = {"schema": 1, "verdict": "infrastructure", "source": {"kind": args.kind, "name": step_name, "code_snapshot_sha256": snapshot}, "outcomes": {"total": 0, "passed": 0, "failed": 0}, "evidence": {"log": str(Path(evidence_path).resolve())}, "error": str(exc)}
        write_atomic(args.out, failure)
        print(f"gate result: {exc}", file=__import__("sys").stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
