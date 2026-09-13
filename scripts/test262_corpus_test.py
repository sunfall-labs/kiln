import json
import os
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
        with mock.patch.object(corpus, "COMMIT", subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()):
            return root

    def test_validate_rejects_dirty_and_empty_checkout(self):
        root = self.valid()
        with self.assertRaises(corpus.CorpusError):
            corpus.validate_checkout(root)
        (root / "test/language/a/test.js").unlink()
        subprocess.run(["git", "-C", str(root), "checkout", "--", "."], check=True)
        (root / "test/language/a/test.js").unlink()
        with mock.patch.object(corpus, "COMMIT", subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()):
            with self.assertRaises(corpus.CorpusError):
                corpus.resolve_test_root(root)

    def test_resolve_selection_is_contained_and_nonempty(self):
        root = self.valid()
        with mock.patch.object(corpus, "COMMIT", subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()):
            selected, report = corpus.resolve_test_root(root, "language/a")
            self.assertEqual(report, "language/a")
            self.assertTrue(selected.is_dir())
            for value in ("../", "", "language/missing"):
                if value == "":
                    continue
                with self.assertRaises(corpus.CorpusError):
                    corpus.resolve_test_root(root, value)

    def test_discovery_and_report_validation(self):
        root = self.valid()
        chunks = corpus.discover_chunks(root / "test")
        self.assertEqual(chunks, ["language/a"])
        with self.assertRaises(corpus.CorpusError):
            corpus.discover_chunks(root / "test", ["language/missing"])
        fake = root / "fake-kiln"
        fake.write_text("#!/usr/bin/env python3\nimport os, pathlib, sys\npathlib.Path(sys.argv[5]).write_text(os.environ['FAKE_REPORT'])\n")
        fake.chmod(0o755)
        raw = root / "result.json"
        log = root / "run.log"
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps({"tests": [{"id": "a", "outcome": "pass"}]})}):
            report, _ = corpus.run_result(str(fake), root / "test/language/a", raw, log)
        self.assertEqual(report["_kiln_counts"]["pass"], 1)
        for invalid in ({}, {"tests": []}, {"tests": [{"id": "a", "outcome": "wat"}]}):
            with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(invalid)}):
                with self.assertRaises(corpus.CorpusError):
                    corpus.run_result(str(fake), root / "test/language/a", raw, log)

    def test_managed_fetch_stages_and_explicit_checkout_is_untouched(self):
        source = self.valid()
        head = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
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
