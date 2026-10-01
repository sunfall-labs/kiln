"""Focused process-lifecycle checks for the opt-in native corpus deadline."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
HELPER = Path(__file__).with_name("native_timeout.py")
RUNNER = Path(__file__).with_name("native.sh")


class NativeTimeoutTests(unittest.TestCase):
    def test_completed_child_keeps_output_and_exit_status(self):
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / "timed-out"
            result = subprocess.run(
                [sys.executable, str(HELPER), "2", str(marker), "--", sys.executable,
                 "-c", "import sys; print('complete'); sys.exit(7)"],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 7)
            self.assertEqual(result.stdout, "complete\n")
            self.assertEqual(result.stderr, "")
            self.assertFalse(marker.exists())

    def test_timeout_keeps_partial_output_and_kills_descendant_group(self):
        with tempfile.TemporaryDirectory() as temporary:
            marker = Path(temporary) / "timed-out"
            descendant_marker = Path(temporary) / "descendant-survived"
            grandchild = (
                "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                "time.sleep(0.8); open(%r, 'w').write('survived')" % str(descendant_marker)
            )
            parent = (
                "import signal, subprocess, sys, time; "
                "subprocess.Popen([sys.executable, '-c', %r]); "
                "print('partial output', flush=True); "
                "signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(30)" % grandchild
            )
            result = subprocess.run(
                [sys.executable, str(HELPER), "1", str(marker), "--", sys.executable,
                 "-c", parent],
                capture_output=True,
                text=True,
                timeout=5,
            )
            self.assertEqual(result.returncode, 124)
            self.assertTrue(marker.exists())
            self.assertIn("partial output\n", result.stdout)
            self.assertIn("KILN_NATIVE_TIMEOUT:", result.stderr)
            time.sleep(1)
            self.assertFalse(descendant_marker.exists(), "the owned process group outlived timeout")

    def test_runner_times_out_one_probe_and_finishes_the_batch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            probes = root / "probes"
            probes.mkdir()
            slow = probes / "slow.ts"
            fast = probes / "fast.ts"
            slow.write_text("// slow\n")
            fast.write_text("// fast\n")
            kiln = root / "kiln"
            kiln.write_text(
                "#!/usr/bin/env python3\n"
                "import sys, time\n"
                "native = '--native' in sys.argv\n"
                "probe = sys.argv[-1]\n"
                "if native and probe.endswith('slow.ts'):\n"
                " print('kiln: native backend refusal: deliberately slow', flush=True)\n"
                " time.sleep(30)\n"
                "print('same')\n"
            )
            kiln.chmod(0o755)
            environment = dict(
                os.environ,
                KILN_BIN=str(kiln),
                KILN_JOBS="1",
                KILN_NATIVE_TIMEOUT_SECONDS="1",
            )
            result = subprocess.run(
                [str(RUNNER), str(slow), str(fast)],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FAILED: deadline expired while running slow", result.stdout)
            self.assertIn("native: 1 matched", result.stdout)
            self.assertIn("none were classified as refusals", result.stdout)
            self.assertNotIn("Refused by the native backend", result.stdout)


if __name__ == "__main__":
    unittest.main()
