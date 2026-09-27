"""Explicit inventories, disjoint suite selection, and bounded Test262 batch scheduling."""
from __future__ import annotations

from concurrent.futures import ProcessPoolExecutor, as_completed
import hashlib
import json
import multiprocessing
import os
from pathlib import Path
import subprocess
import time

from test262_memory import available_cpus, child_environment, limit_bytes, run_limited, worker_count
from test262_corpus import COMMIT, CorpusError, OUTCOMES, initialize_chunk_worker, run_result


def load_extended(path, baseline):
    raw = Path(path).read_bytes()
    document = json.loads(raw)
    if document.get("schema") != 1 or document.get("test262_commit") != COMMIT:
        raise CorpusError("extended manifest schema or Test262 pin does not match")
    chunks = document.get("chunks")
    if not isinstance(chunks, dict) or not chunks:
        raise CorpusError("extended manifest has no chunks")
    if not isinstance(document.get("inventory"), dict) or set(document["inventory"]) != set(baseline["chunks"]):
        raise CorpusError("extended manifest must pin the complete chunk inventory")
    for chunk, entry in chunks.items():
        if chunk not in baseline["chunks"]:
            raise CorpusError(f"extended manifest names unknown chunk {chunk}")
        if not isinstance(entry.get("files"), dict) or not entry["files"]:
            raise CorpusError(f"extended chunk {chunk} has no files")
        for name, cost in entry["files"].items():
            if not name or not isinstance(cost, int) or isinstance(cost, bool) or cost <= 0:
                raise CorpusError(f"invalid extended file/timing: {chunk}/{name}")
        for key in ("passed", "failed", "unsupported", "skipped"):
            value = entry.get("expected", {}).get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise CorpusError(f"invalid extended expected {chunk}.{key}")
        if entry["expected"]["passed"] > baseline["chunks"][chunk]["passed"]:
            raise CorpusError(f"extended passing baseline exceeds full baseline for {chunk}")
    return document, hashlib.sha256(raw).hexdigest()


def inventory_identity(files):
    cases = sorted(case for variants in files.values() for case in variants)
    if len(cases) != len(set(cases)):
        raise CorpusError("duplicate cases in inventory")
    return {"files": len(files), "cases": len(cases),
            "case_ids_sha256": hashlib.sha256(("\n".join(cases) + "\n").encode()).hexdigest()}


def inventory(kiln, root, logdir, chunk):
    prefix = Path(logdir) / chunk.replace("/", "_")
    output = Path(str(prefix) + ".inventory.json")
    log = Path(str(prefix) + ".inventory.log")
    output.unlink(missing_ok=True)
    with log.open("w") as stream:
        completed, exceeded = run_limited(
            [kiln, "--one-shot", "test", str(Path(root) / chunk), "--list", "--out", str(output)],
            stdout=stream, stderr=subprocess.STDOUT, env=child_environment(os.environ),
        )
    if exceeded or completed.returncode != 0 or not output.is_file():
        raise CorpusError(f"inventory failed for {chunk}; full log: {log}")
    entries = json.loads(output.read_text())
    if not isinstance(entries, list) or not entries:
        raise CorpusError(f"empty or invalid inventory for {chunk}")
    files, cases = {}, set()
    for entry in entries:
        name = entry.get("id") if isinstance(entry, dict) else None
        variants = entry.get("cases") if isinstance(entry, dict) else None
        if not isinstance(name, str) or not name or name in files or not isinstance(variants, list) or not variants:
            raise CorpusError(f"invalid or duplicate file in {chunk} inventory")
        for case in variants:
            if not isinstance(case, str) or not case or case in cases:
                raise CorpusError(f"invalid or duplicate case in {chunk} inventory")
            cases.add(case)
        files[name] = variants
    return chunk, files


def partition(inventories, extended, suite):
    """The pinned inventory is the authority; timings never select or drop a case."""
    if suite not in {"routine", "extended", "full"}:
        raise CorpusError(f"unknown Test262 suite {suite}")
    selected, sections = {}, {}
    discovered = deferred = 0
    if set(inventories) != set(extended.get("inventory", {})):
        raise CorpusError("discovered chunk inventory differs from the pinned manifest")
    for chunk, files in inventories.items():
        if inventory_identity(files) != extended["inventory"][chunk]:
            raise CorpusError(f"discovered file/variant inventory differs from pinned IDs for {chunk}")
        slow = extended["chunks"].get(chunk, {}).get("files", {})
        missing = slow.keys() - files.keys()
        if missing:
            raise CorpusError(f"extended manifest has undiscovered IDs in {chunk}: {sorted(missing)}")
        chosen = {}
        for name, cases in files.items():
            section = "extended" if name in slow else "routine"
            sections[(chunk, name)] = section
            discovered += len(cases)
            if suite == "full" or suite == section:
                chosen[name] = cases
            else:
                deferred += len(cases)
        if chosen:
            selected[chunk] = chosen
    if not selected:
        raise CorpusError(f"Test262 {suite} selection is empty")
    return selected, sections, {"suite": suite, "discovered_cases": discovered,
                                "selected_cases": discovered - deferred, "deferred_cases": deferred,
                                "verified_partition": True}


def batches(selected, extended, workers):
    tasks = []
    for chunk, files in selected.items():
        costs = extended["chunks"].get(chunk, {}).get("files", {})
        order = sorted(files, key=lambda name: (-costs.get(name, 1), name))
        batch, cost = [], 0
        for name in order:
            batch.append(name)
            cost += costs.get(name, 1)
            # Keep enough files in a batch to occupy its workers even when one file is slow.
            # The size ceiling also bounds quick-case discovery/startup amortization.
            if len(batch) >= 256 or (cost >= 20_000 and len(batch) >= workers * 2):
                tasks.append((cost, chunk, batch))
                batch, cost = [], 0
        if batch:
            tasks.append((cost, chunk, batch))
    return sorted(tasks, key=lambda task: (-task[0], task[1], task[2][0]))


def execute_batch(kiln, root, logdir, chunk, number, ids):
    prefix = Path(logdir) / f"batch-{number:04d}"
    selection = prefix.with_suffix(".ids.json")
    selection.write_text(json.dumps(ids) + "\n")
    started = time.monotonic()
    report, _ = run_result(kiln, Path(root) / chunk, prefix.with_suffix(".results.json"),
                           prefix.with_suffix(".log"), verify_cap=False, cases=selection)
    return chunk, ids, str(prefix.with_suffix(".results.json")), started, time.monotonic()


def verify_batch(report, expected):
    actual = [test["id"] for test in report["tests"]]
    if len(actual) != len(set(actual)) or set(actual) != set(expected):
        raise CorpusError("batch coverage mismatch: missing, extra or duplicate case IDs")


def schedule(args, root, chunks, baseline):
    extended, digest = load_extended(args.extended_manifest, baseline)
    workers = worker_count(os.environ, available_cpus())
    lanes = min(4, max(1, workers // 4)) if os.environ.get("KILN_GATE_SERIAL") != "1" else 1
    jobs = workers // lanes
    environment = dict(os.environ, KILN_JOBS=str(jobs), RAYON_NUM_THREADS=str(jobs),
                       KILN_TEST262_MEMORY_MB=str(limit_bytes() // (1024 * 1024) // lanes))
    print(f"Test262 {args.suite}: {lanes} batch processes, {jobs} workers and "
          f"{environment['KILN_TEST262_MEMORY_MB']} MiB per process", flush=True)
    Path(args.logdir).mkdir(parents=True, exist_ok=True)
    with ProcessPoolExecutor(max_workers=lanes, mp_context=multiprocessing.get_context("spawn"),
                             initializer=initialize_chunk_worker, initargs=(environment,)) as pool:
        pending = [pool.submit(inventory, args.kiln, root, args.logdir, chunk) for chunk in chunks]
        inventories = dict(result.result() for result in as_completed(pending))
        selected, sections, coverage = partition(inventories, extended, args.suite)
        coverage["extended_manifest_sha256"] = digest
        args.coverage = coverage
        args.extended_document = extended
        args.selected_chunks = sorted(selected)
        tasks = batches(selected, extended, jobs)
        plan = {"schema": 1, "test262_commit": COMMIT, "coverage": coverage,
                "processes": lanes, "workers_per_process": jobs,
                "memory_mb_per_process": int(environment["KILN_TEST262_MEMORY_MB"]),
                "batches": [{"number": number, "chunk": chunk, "files": ids,
                             "estimated_millis": cost,
                             "expected_cases": sum(len(selected[chunk][name]) for name in ids)}
                            for number, (cost, chunk, ids) in enumerate(tasks)]}
        (Path(args.logdir) / "plan.json").write_text(json.dumps(plan, indent=2) + "\n")
        print(f"coverage: {coverage['selected_cases']} selected, {coverage['deferred_cases']} deferred, "
              f"{coverage['discovered_cases']} discovered; {len(tasks)} batches", flush=True)
        all_tests = {chunk: {} for chunk in selected}
        spans = {}
        pending = [pool.submit(execute_batch, args.kiln, root, args.logdir, chunk, number, ids)
                   for number, (_, chunk, ids) in enumerate(tasks)]
        for future in as_completed(pending):
            chunk, ids, path, started, ended = future.result()
            report = json.loads(Path(path).read_text())
            expected = [case for name in ids for case in selected[chunk][name]]
            verify_batch(report, expected)
            if all_tests[chunk].keys() & set(expected):
                raise CorpusError(f"duplicate batch coverage for {chunk}")
            all_tests[chunk].update((test["id"], test) for test in report["tests"])
            lo, hi = spans.get(chunk, (started, ended))
            spans[chunk] = min(lo, started), max(hi, ended)
    for chunk in sorted(selected):
        expected = {case for cases in selected[chunk].values() for case in cases}
        if set(all_tests[chunk]) != expected:
            raise CorpusError(f"incomplete batch union for {chunk}")
        tests = sorted(all_tests[chunk].values(), key=lambda test: test["id"])
        counts = {key: sum(test["outcome"] == key for test in tests) for key in OUTCOMES}
        totals = dict(total=len(tests), passed=counts["pass"], failed=counts["fail"],
                      unsupported=counts["unsupported"], skipped=counts["skipped"])
        section_counts = {}
        for section in ("routine", "extended"):
            section_ids = {case for name, cases in selected[chunk].items()
                           if sections[(chunk, name)] == section for case in cases}
            section_counts[section] = {key: sum(test["id"] in section_ids and test["outcome"] == key for test in tests)
                                       for key in OUTCOMES}
        result = {"schema": 1, "runner": "kiln Test262 batches", "totals": totals, "tests": tests,
                  "coverage": {"suite": args.suite, "selected_cases": len(tests),
                               "discovered_cases": sum(map(len, inventories[chunk].values()))}}
        (Path(args.logdir) / (chunk.replace("/", "_") + ".results.json")).write_text(json.dumps(result, indent=2) + "\n")
        yield chunk, {"totals": totals, "_kiln_counts": counts, "_kiln_section_counts": section_counts,
                      "_kiln_elapsed_millis": max(1, round((spans[chunk][1] - spans[chunk][0]) * 1000))}
