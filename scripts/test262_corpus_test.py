import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import test262_corpus as corpus


class CorpusPolicyTests(unittest.TestCase):
    def checkout(self, files=None):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, ignore_errors=True)
        (root / "harness").mkdir()
        (root / "harness/assert.js").write_text("// assert\n")
        (root / "test/language/a").mkdir(parents=True)
        for name, body in (files or {"test/language/a/test.js": "print(1);\n"}).items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        subprocess.run(["git", "init", "--quiet", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", "."], check=True)
        subprocess.run(["git", "-C", str(root), "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "--quiet", "-m", "test"], check=True)
        return root

    def valid(self):
        root = self.checkout()
        return root, subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()

    def test_validate_rejects_dirty_and_empty_checkout(self):
        root, head = self.valid()
        with mock.patch.object(corpus, "COMMIT", "wrong"):
            with self.assertRaisesRegex(corpus.CorpusError, "expected pinned commit"):
                corpus.validate_checkout(root)
        with mock.patch.object(corpus, "COMMIT", head):
            (root / "dirty.txt").write_text("dirty")
            with self.assertRaisesRegex(corpus.CorpusError, "dirty"):
                corpus.validate_checkout(root)
            (root / "dirty.txt").unlink()
        (root / "test/language/a/test.js").unlink()
        subprocess.run(["git", "-C", str(root), "checkout", "--", "."], check=True)
        (root / "test/language/a/test.js").unlink()
        with mock.patch.object(corpus, "COMMIT", head):
            with self.assertRaisesRegex(corpus.CorpusError, "empty"):
                corpus.resolve_test_root(root)

    def test_resolve_selection_is_contained_and_nonempty(self):
        root, head = self.valid()
        with mock.patch.object(corpus, "COMMIT", head):
            selected, report = corpus.resolve_test_root(root, "language/a")
            self.assertEqual(report, "language/a")
            self.assertTrue(selected.is_dir())
            outside = Path(tempfile.mkdtemp())
            self.addCleanup(shutil.rmtree, outside, ignore_errors=True)
            (outside / "escape.js").write_text("x")
            (root / "test/escape").symlink_to(outside, target_is_directory=True)
            for value in ("../", "language/missing", "escape"):
                with self.assertRaisesRegex(corpus.CorpusError, "path|subtree"):
                    corpus.resolve_test_root(root, value)

    def test_discovery_and_report_validation(self):
        root, head = self.valid()
        chunks = corpus.discover_chunks(root / "test")
        self.assertEqual(chunks, ["language/a"])
        with self.assertRaises(corpus.CorpusError):
            corpus.discover_chunks(root / "test", ["language/missing"])
        fake = root / "fake-kiln"
        fake.write_text("#!/usr/bin/env python3\nimport os, pathlib, sys\nif os.environ.get('FAKE_NO_WRITE') != '1': pathlib.Path(sys.argv[5]).write_text(os.environ['FAKE_REPORT'])\n")
        fake.chmod(0o755)
        raw = root / "result.json"
        log = root / "run.log"
        valid = {"tests": [{"id": "a", "outcome": "pass"}], "totals": {"total": 1, "passed": 1, "failed": 0, "unsupported": 0, "skipped": 0}}
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(valid), "FAKE_NO_WRITE": "0"}):
            report, _ = corpus.run_result(str(fake), root / "test/language/a", raw, log)
        self.assertEqual(report["_kiln_counts"]["pass"], 1)
        invalid_reports = ({}, {"tests": []}, {"tests": [{"id": "a", "outcome": "wat"}]}, {"tests": [{"id": "a", "outcome": "pass"}], "totals": {"total": 2, "passed": 2, "failed": 0, "unsupported": 0, "skipped": 0}})
        for invalid in invalid_reports:
            with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(invalid), "FAKE_NO_WRITE": "0"}):
                with self.assertRaises(corpus.CorpusError):
                    corpus.run_result(str(fake), root / "test/language/a", raw, log)
        raw.write_text(json.dumps(valid))
        with mock.patch.dict(os.environ, {"FAKE_NO_WRITE": "1", "FAKE_REPORT": json.dumps(valid)}):
            with self.assertRaises(corpus.CorpusError):
                corpus.run_result(str(fake), root / "test/language/a", raw, log)

    def test_managed_fetch_stages_and_explicit_checkout_is_untouched(self):
        source, head = self.valid()
        destination = Path(tempfile.mkdtemp()) / "managed"
        with mock.patch.object(corpus, "COMMIT", head), mock.patch.object(corpus, "REPOSITORY", f"file://{source}"):
            fetched = corpus.fetch_managed(destination, minimum_files=1)
            self.assertEqual(fetched, destination.resolve())
            self.assertEqual((fetched / "harness/assert.js").read_text(), "// assert\n")
            marker = destination / "user-marker"
            marker.write_text("keep")
            with mock.patch.dict(os.environ, {"KILN_TEST262_CHECKOUT": str(destination)}):
                with self.assertRaises(corpus.CorpusError):
                    corpus.command_fetch(type("Args", (), {"destination": None})())
            self.assertTrue(marker.exists())


if __name__ == "__main__":
    unittest.main()
