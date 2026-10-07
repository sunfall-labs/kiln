#!/usr/bin/env python3
"""Compare the archived 322-file interpreter report with a compressed 7-file replay."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
from pathlib import Path
from typing import Any


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_report(path: Path) -> tuple[dict[str, Any], bytes]:
    raw = path.read_bytes()
    payload = gzip.decompress(raw) if path.suffix == ".gz" else raw
    return json.loads(payload), payload


def cases_by_file(report: dict[str, Any]) -> dict[str, list[dict[str, Any]]]:
    result: dict[str, list[dict[str, Any]]] = {}
    for file_report in report["files"]:
        result[file_report["file"]] = file_report["engines"]["interpreter"].get("cases", [])
    return result


def qualified(file: str, case_id: str) -> str:
    return f"{file}: {case_id}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("full_report", type=Path)
    parser.add_argument("cohort_control_report", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    full, full_payload = read_report(args.full_report)
    cohort, cohort_payload = read_report(args.cohort_control_report)
    full_cases = cases_by_file(full)
    cohort_cases = cases_by_file(cohort)
    cohort_files = sorted(cohort_cases)
    full_failures = {
        qualified(file, case["id"])
        for file, cases in full_cases.items()
        for case in cases
        if case.get("outcome") == "fail"
    }
    cohort_failures = {
        qualified(file, case["id"])
        for file, cases in cohort_cases.items()
        for case in cases
        if case.get("outcome") == "fail"
    }
    cohort_case_ids = sorted(
        qualified(file, case["id"])
        for file, cases in cohort_cases.items()
        for case in cases
        if case.get("kind") == "case"
    )
    full_inventory = {
        qualified(file, case["id"])
        for file, cases in full_cases.items()
        for case in cases
        if case.get("kind") == "case"
    }
    expected_files = {
        "Cron",
        "Hash",
        "arbitrary/Arbitrary",
        "cluster/ClusterWorkflowEngine",
        "cluster/Sharding",
        "encoding/SchemaBinary",
        "schema/toDifferJsonPatch",
    }
    if set(cohort_files) != expected_files or len(cohort_case_ids) != 562:
        raise SystemExit("cohort inventory differs from the archived seven-file/562-case scope")
    if full["pin"] != cohort["pin"]:
        raise SystemExit("full and cohort reports use different upstream pins")
    if not set(cohort_case_ids) <= full_inventory:
        raise SystemExit("cohort case IDs are not all present in the full report inventory")

    full_aggregate = full["aggregate"]["engines"]["interpreter"]
    cohort_aggregate = cohort["aggregate"]["engines"]["interpreter"]
    full_oracle = full.get("node_vitest_oracle", {})
    cohort_oracle = cohort.get("node_vitest_oracle", {})
    full_harness_digest = sha256_bytes(
        json.dumps(full.get("harness_sha256", {}), sort_keys=True, separators=(",", ":")).encode()
    )
    cohort_harness_digest = sha256_bytes(
        json.dumps(cohort.get("harness_sha256", {}), sort_keys=True, separators=(",", ":")).encode()
    )
    audit = {
        "scope": {
            "description": "Full 322-file report compared only against a later exact replay of seven files.",
            "full_files": full["selected"],
            "cohort_files": cohort_files,
            "cohort_cases": len(cohort_case_ids),
            "full_files_not_replayed": full["selected"] - len(cohort_files),
            "outside_cohort_failures": sorted(
                case for case in full_failures if case.split(": ", 1)[0] not in expected_files
            ),
            "outside_cohort_status": "none in the archived full report; remaining 315 files were not rerun in the later cohort",
        },
        "upstream": {
            "pin": full["pin"],
            "full_pin_matches_cohort": full["pin"] == cohort["pin"],
            "reference": full["reference"],
            "node_vitest_version": full_oracle.get("vitest"),
            "node_version": full_oracle.get("node"),
            "oracle_sha256": full_oracle.get("sha256"),
            "cohort_oracle_sha256": cohort_oracle.get("sha256"),
            "runner_sha256": full.get("runner_sha256"),
            "cohort_runner_sha256": cohort.get("runner_sha256"),
            "harness_manifest_sha256": full_harness_digest,
            "cohort_harness_manifest_sha256": cohort_harness_digest,
        },
        "inputs": {
            "full_report_path": str(args.full_report),
            "full_report_sha256": sha256_bytes(full_payload),
            "full_cli_sha256": full["kiln_binary_sha256"],
            "cohort_report_path": str(args.cohort_control_report),
            "cohort_report_file_sha256": sha256_bytes(args.cohort_control_report.read_bytes()),
            "cohort_report_uncompressed_sha256": sha256_bytes(cohort_payload),
            "cohort_cli_sha256": cohort["kiln_binary_sha256"],
            "cohort_source_sha256": cohort.get("source_sha256", {}),
        },
        "results": {
            "full_summary": {
                key: full_aggregate[key]
                for key in ("complete_files", "incomplete_files", "registered_cases", "recorded_cases", "pass", "fail", "skip", "node_vitest_mapped_cases", "node_vitest_unmapped_cases")
            },
            "cohort_summary": {
                key: cohort_aggregate[key]
                for key in ("complete_files", "incomplete_files", "registered_cases", "recorded_cases", "pass", "fail", "skip", "node_vitest_mapped_cases", "node_vitest_unmapped_cases")
            },
            "cohort_case_ids": cohort_case_ids,
            "full_failure_ids": sorted(full_failures),
            "cohort_failure_ids": sorted(cohort_failures),
            "old_failures_passing_in_cohort": sorted(full_failures - cohort_failures),
        },
    }
    rendered = json.dumps(audit, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered)
    else:
        print(rendered, end="")


if __name__ == "__main__":
    main()
