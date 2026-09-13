"""Subprocess integration tests for the gate result CLI.

These tests deliberately exercise the command line boundary rather than importing
``gate_results``.  The gate consumes files produced by separate processes, so the
fixtures include a real git index and a real binary whose digest is supplied to the
CLI through the same environment variable used by ``gate.sh``.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("gate_results.py").resolve()


class GateResultsCliIntegrationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="kiln-gate-results-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.env = os.environ.copy()
        self.env.pop("KILN_CODE_SNAPSHOT_SHA256", None)
        self.env["PYTHONDONTWRITEBYTECODE"] = "1"
        self.env["KILN_GATE_MIN_PASSED"] = "0"
        self._run_git("init", "--quiet")
        self._run_git("config", "user.email", "gate-results@example.invalid")
        self._run_git("config", "user.name", "Gate Results Tests")
        (self.root / "fixture.bin").write_bytes(b"gate-results-binary\x00\xff\n")
        self._run_git("add", "fixture.bin")
        self.binary = self.root / "kiln"
        self.binary.write_bytes(b"tiny real executable fixture\n")
        self.env["KILN_BIN"] = str(self.binary)

    def _run_git(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args],
            cwd=self.root,
            env=self.env,
            text=True,
            capture_output=True,
            check=True,
        )

    def cli(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), *args],
            cwd=self.root,
            env=self.env,
            text=True,
            capture_output=True,
        )

    def process(self, name: str, status: int = 0) -> Path:
        log = self.root / f"{name.replace(' ', '-')}.log"
        log.write_text(f"{name}: status {status}\n", encoding="utf-8")
        out = self.root / f"{name.replace(' ', '-')}.envelope.json"
        result = self.cli(
            "process",
            "--name",
            name,
            "--log",
            str(log),
            "--status",
            str(status),
            "--out",
            str(out),
        )
        self.assertEqual(result.returncode, 0 if status == 0 else 1, result.stderr)
        self.assertTrue(out.exists(), result.stdout + result.stderr)
        return out

    def workspace_step(self) -> Path:
        log = self.root / "crate-tests.log"
        log.write_text(
            "test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
            encoding="utf-8",
        )
        out = self.root / "crate-tests.envelope.json"
        result = self.cli(
            "workspace",
            "--name",
            "crate tests",
            "--log",
            str(log),
            "--status",
            "0",
            "--out",
            str(out),
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return out

    def aggregate(self, expected: list[str]) -> subprocess.CompletedProcess[str]:
        out = self.root / "aggregate.json"
        args = ["aggregate", "--directory", str(self.root), "--out", str(out)]
        for name in expected:
            args.extend(("--expected", name))
        result = self.cli(*args)
        if out.exists():
            self.aggregate_document = json.loads(out.read_text(encoding="utf-8"))
        return result

    def test_quick_four_steps_pass_and_preserve_step_outcomes(self) -> None:
        names = ["build", "crate tests", "native parity", "thirdparty"]
        paths = [self.process("build"), self.workspace_step(), self.process("native parity"), self.process("thirdparty")]
        result = self.aggregate(names)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.aggregate_document["verdict"], "passed")
        self.assertEqual(sorted(self.aggregate_document["evidence"]["steps"]), sorted(names))
        self.assertEqual(sorted(step["source"]["name"] for step in self.aggregate_document["steps"]), sorted(names))
        for path in paths:
            document = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(document["verdict"], "passed")
            self.assertEqual(document["source"]["code_snapshot_sha256"], self.aggregate_document["source"]["code_snapshot_sha256"])
            self.assertEqual(
                document["source"]["binary_sha256"],
                hashlib.sha256(self.binary.read_bytes()).hexdigest(),
            )
        workspace_record = next(record for record in self.aggregate_document["steps"] if record["source"]["name"] == "crate tests")
        self.assertEqual(workspace_record["outcomes"], {"total": 3, "passed": 2, "failed": 0})
        self.assertEqual(workspace_record["outcome_breakdown"]["ignored"], 1)
        self.assertEqual(workspace_record["evidence"]["log"], str((self.root / "crate-tests.log").resolve()))

    def test_missing_duplicate_and_malformed_steps_fail(self) -> None:
        self.process("build")
        self.process("crate tests")
        self.assertNotEqual(self.aggregate(["build", "crate tests", "native parity"]).returncode, 0)

        duplicate = self.root / "duplicate.envelope.json"
        duplicate.write_text((self.root / "build.envelope.json").read_text(encoding="utf-8"), encoding="utf-8")
        self.assertNotEqual(self.aggregate(["build", "crate tests", "build"]).returncode, 0)

        duplicate.unlink()
        (self.root / "malformed.envelope.json").write_text("{not json\n", encoding="utf-8")
        self.assertNotEqual(self.aggregate(["build", "crate tests"]).returncode, 0)

    def test_mixed_snapshot_and_binary_fail(self) -> None:
        first = self.process("build")
        second = self.process("native parity")
        first_doc = json.loads(first.read_text(encoding="utf-8"))
        second_doc = json.loads(second.read_text(encoding="utf-8"))

        second_doc["source"]["code_snapshot_sha256"] = "different-code-snapshot"
        second.write_text(json.dumps(second_doc), encoding="utf-8")
        self.assertNotEqual(self.aggregate(["build", "native parity"]).returncode, 0)

        second_doc["source"]["code_snapshot_sha256"] = first_doc["source"]["code_snapshot_sha256"]
        first_doc["source"]["binary_sha256"] = "binary-a"
        second_doc["source"]["binary_sha256"] = "binary-b"
        first.write_text(json.dumps(first_doc), encoding="utf-8")
        second.write_text(json.dumps(second_doc), encoding="utf-8")
        self.assertNotEqual(self.aggregate(["build", "native parity"]).returncode, 0)

    def test_process_failure_and_build_failure_cannot_be_overridden(self) -> None:
        report = self.root / "test262-report.json"
        report.write_text(
            json.dumps(
                {
                    "schema": 1,
                    "status": "passed",
                    "expected_chunks": ["chunk-0"],
                    "chunks": {"chunk-0": {"total": 1, "passed": 1, "failed": 0, "unsupported": 0, "skipped": 0}},
                    "counts": {"total": 1, "passed": 1, "failed": 0, "unsupported": 0, "skipped": 0},
                    "source": {"commit": "fixture-pin", "name": "test262 fixture"},
                }
            ),
            encoding="utf-8",
        )
        log = self.root / "test262.log"
        log.write_text("test262 child exited non-zero\n", encoding="utf-8")
        out = self.root / "test262.envelope.json"
        result = self.cli(
            "test262",
            "--name",
            "test262 baseline",
            "--log",
            str(log),
            "--status",
            "1",
            "--document",
            str(report),
            "--out",
            str(out),
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        envelope = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(envelope["verdict"], "infrastructure")
        self.assertIn("exited 1 despite a passing report", envelope["error"])

        out.unlink()
        self.process("build", status=1)
        self.process("crate tests")
        result = self.aggregate(["build", "crate tests"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.aggregate_document["verdict"], "failed")
        self.assertIn("build", self.aggregate_document["failed_steps"])

        result = self.aggregate(["build", "crate tests", "native parity"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.aggregate_document["verdict"], "infrastructure")


if __name__ == "__main__":
    unittest.main()
