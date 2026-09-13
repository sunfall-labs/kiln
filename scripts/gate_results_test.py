import json
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

import gate_results as results


class GateResultTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.previous_floor = os.environ.get("KILN_GATE_MIN_PASSED")
        self.previous_binary = os.environ.get("KILN_BIN")
        os.environ["KILN_GATE_MIN_PASSED"] = "0"
        os.environ["KILN_BIN"] = sys.executable
        self.addCleanup(self._restore_floor)

    def _restore_floor(self):
        if self.previous_floor is None:
            os.environ.pop("KILN_GATE_MIN_PASSED", None)
        else:
            os.environ["KILN_GATE_MIN_PASSED"] = self.previous_floor
        if self.previous_binary is None:
            os.environ.pop("KILN_BIN", None)
        else:
            os.environ["KILN_BIN"] = self.previous_binary

    def log(self, text):
        path = self.root / "run.log"
        path.write_text(text)
        return str(path)

    def test_workspace_requires_summary_and_preserves_build_failure(self):
        with self.assertRaises(results.ResultError):
            results.workspace(self.log("error: failed to compile"), 1)
        doc = results.workspace(self.log("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n"), 0)
        self.assertEqual(doc["outcomes"]["passed"], 2)

    def test_envelope_rejects_empty_and_inconsistent_counts(self):
        for counts in ({"total": 0, "passed": 0, "failed": 0}, {"total": 1, "passed": 2, "failed": 0}):
            with self.assertRaises(results.ResultError):
                results.validate({"schema": 1, "verdict": "passed", "source": {"kind": "x", "code_snapshot_sha256": "snapshot"}, "outcomes": counts, "evidence": {"log": "run.log"}})

    def test_conformance_reads_structured_document(self):
        log = self.log("pass g01\nfail g02\n")
        document = self.root / "results.json"
        document.write_text(json.dumps({"schema": 1, "tests": [{"id": "g01", "outcome": "pass"}, {"id": "g02", "outcome": "fail"}]}))
        doc = results.conformance(log, 1, str(document))
        self.assertEqual(doc["outcomes"], {"total": 2, "passed": 1, "failed": 1, "unsupported": 0, "skipped": 0})

    def test_process_aggregate_rejects_missing_and_mixed_snapshots(self):
        for name, snapshot in (("build", "a"), ("tests", "b")):
            (self.root / f"{name}.envelope.json").write_text(json.dumps({"schema": 1, "verdict": "passed", "measurement": "process", "source": {"kind": "process", "name": name, "code_snapshot_sha256": snapshot}, "evidence": {"log": "run.log"}}))
        with self.assertRaises(results.ResultError):
            results.aggregate(str(self.root), ["build", "tests"], str(self.root / "aggregate.json"))


if __name__ == "__main__":
    unittest.main()
