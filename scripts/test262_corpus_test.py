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
import test262_memory


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
        fake.write_text("#!/usr/bin/env python3\nimport os, pathlib, sys\nif os.environ.get('FAKE_NO_WRITE') != '1': pathlib.Path(sys.argv[5]).write_text(os.environ['FAKE_REPORT'])\nsys.exit(int(os.environ.get('FAKE_EXIT', '0')))\n")
        fake.chmod(0o755)
        raw = root / "result.json"
        log = root / "run.log"
        valid = {"tests": [{"id": "a", "outcome": "pass"}], "totals": {"total": 1, "passed": 1, "failed": 0, "unsupported": 0, "skipped": 0}}
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(valid), "FAKE_NO_WRITE": "0"}):
            report, _ = corpus.run_result(str(fake), root / "test/language/a", raw, log)
        self.assertEqual(report["_kiln_counts"]["pass"], 1)
        failing = {"tests": [{"id": "a", "outcome": "fail"}], "totals": {"total": 1, "passed": 0, "failed": 1, "unsupported": 0, "skipped": 0}}
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(failing), "FAKE_NO_WRITE": "0", "FAKE_EXIT": "1"}):
            report, completed = corpus.run_result(str(fake), root / "test/language/a", raw, log)
        self.assertEqual((report["_kiln_counts"]["fail"], completed.returncode), (1, 1))
        unsupported = {"tests": [{"id": "a", "outcome": "unsupported"}], "totals": {"total": 1, "passed": 0, "failed": 0, "unsupported": 1, "skipped": 0}}
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(unsupported), "FAKE_NO_WRITE": "0", "FAKE_EXIT": "1"}):
            report, completed = corpus.run_result(str(fake), root / "test/language/a", raw, log)
        self.assertEqual((report["_kiln_counts"]["unsupported"], completed.returncode), (1, 1))
        with mock.patch.dict(os.environ, {"FAKE_REPORT": json.dumps(valid), "FAKE_NO_WRITE": "0", "FAKE_EXIT": "9"}):
            with self.assertRaisesRegex(corpus.CorpusError, "exited 9"):
                corpus.run_result(str(fake), root / "test/language/a", raw, log)
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

    def test_managed_fetch_rejects_live_and_broken_destination_symlinks(self):
        source, head = self.valid()
        destination_root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, destination_root, ignore_errors=True)
        target = destination_root / "target"
        target.write_text("preserve")
        live = destination_root / "live"
        live.symlink_to(target)
        broken = destination_root / "broken"
        broken.symlink_to(destination_root / "missing")
        with mock.patch.object(corpus, "COMMIT", head), mock.patch.object(corpus, "REPOSITORY", f"file://{source}"):
            for link in (live, broken):
                with self.assertRaisesRegex(corpus.CorpusError, "symlink"):
                    corpus.fetch_managed(link, minimum_files=1)
        self.assertEqual(target.read_text(), "preserve")


class MemoryPolicyTests(unittest.TestCase):
    def test_worker_count_uses_cpu_job_and_memory_limits(self):
        self.assertEqual(test262_memory.worker_count({}, 2), 2)
        self.assertEqual(test262_memory.worker_count({}, 0), 1)
        self.assertEqual(
            test262_memory.worker_count({"RAYON_NUM_THREADS": "2"}, 32), 2
        )
        self.assertEqual(
            test262_memory.worker_count({"KILN_TEST262_MEMORY_MB": "4096"}, 32), 4
        )
        self.assertEqual(
            test262_memory.worker_count(
                {
                    "KILN_TEST262_MEMORY_MB": "8192",
                    "KILN_JOBS": "3",
                    "RAYON_NUM_THREADS": "6",
                },
                32,
            ),
            3,
        )
        self.assertEqual(
            test262_memory.worker_count(
                {"KILN_TEST262_MEMORY_MB": "512", "KILN_JOBS": "8"}, 32
            ),
            1,
        )

    def test_worker_count_ignores_nonpositive_or_invalid_overrides(self):
        environment = {
            "KILN_TEST262_MEMORY_MB": "4096",
            "KILN_JOBS": "0",
            "RAYON_NUM_THREADS": "not-a-number",
        }
        self.assertEqual(test262_memory.worker_count(environment, 32), 4)

    def test_worker_count_clamps_huge_overrides_by_cpu_and_memory(self):
        environment = {
            "KILN_TEST262_MEMORY_MB": "4096",
            "KILN_JOBS": "999999999999999999999999",
            "RAYON_NUM_THREADS": "999999999999999999999999",
        }
        self.assertEqual(test262_memory.worker_count(environment, 128), 4)

    def test_child_environment_preserves_inputs_and_sets_the_bounded_pool(self):
        environment = {
            "KILN_TEST262_MEMORY_MB": "4096",
            "KILN_JOBS": "3",
            "PRESERVED": "yes",
        }
        child = test262_memory.child_environment(environment, cpu_count=32)
        self.assertEqual(child["RAYON_NUM_THREADS"], "3")
        self.assertEqual(child["PRESERVED"], "yes")
        self.assertNotIn("RAYON_NUM_THREADS", environment)

    def test_run_result_passes_bounded_environment_without_mutating_parent(self):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, ignore_errors=True)
        output = root / "result.json"
        log = root / "run.log"
        report = {
            "tests": [{"id": "a", "outcome": "pass"}],
            "totals": {
                "total": 1,
                "passed": 1,
                "failed": 0,
                "unsupported": 0,
                "skipped": 0,
            },
        }
        captured = {}

        def run_limited(argv, **kwargs):
            captured["environment"] = kwargs["env"]
            Path(argv[5]).write_text(json.dumps(report))
            return subprocess.CompletedProcess(argv, 0), False

        parent_environment = dict(os.environ)
        expected_child_environment = test262_memory.child_environment(parent_environment)
        with (
            mock.patch.object(corpus, "verify_effective_cap"),
            mock.patch.object(corpus, "run_limited", side_effect=run_limited),
        ):
            result, _ = corpus.run_result("fake-kiln", root, output, log)

        self.assertEqual(result["_kiln_counts"]["pass"], 1)
        self.assertEqual(captured["environment"], expected_child_environment)
        self.assertEqual(dict(os.environ), parent_environment)


if __name__ == "__main__":
    unittest.main()
