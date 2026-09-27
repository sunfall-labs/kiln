"""Resource bounds and real stream/exit-status comparisons for the fast gate."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from gate_budget import budget


class BudgetTests(unittest.TestCase):
    def test_lanes_share_one_cpu_budget_and_keep_a_memory_cap(self):
        for cpus in (1, 2, 3, 4, 8, 16, 64):
            for memory in (512, 4096, 32768):
                parallel, total, tests, native, test262, cap = budget({}, cpus, memory)
                self.assertLessEqual(total, cpus)
                self.assertLessEqual(cap, memory // 2)
                self.assertLessEqual(test262, max(1, cap // 1024))
                if parallel:
                    self.assertLessEqual(tests + native + test262, total)
                self.assertGreaterEqual(min(tests, native, test262), 1)

    def test_explicit_limits_are_preserved_and_serial_is_available(self):
        env = {"KILN_JOBS": "6", "KILN_TEST262_MEMORY_MB": "1024"}
        parallel, total, tests, native, test262, cap = budget(env, 32, 32768)
        self.assertEqual((total, test262, cap), (6, 1, 1024))
        self.assertEqual(budget({"KILN_GATE_SERIAL": "1"}, 32, 32768)[0], 0)
        parallel, total, tests, native, _, _ = budget({}, 32, 32768, quick=True)
        self.assertLessEqual(tests + native, total)
        for key in ("KILN_JOBS", "KILN_GATE_JOBS", "RAYON_NUM_THREADS"):
            with self.assertRaises(ValueError):
                budget({key: "0"}, 16, 16384)


@unittest.skipUnless(shutil.which("node"), "Node oracle is not installed")
class ThirdpartyExecutionTests(unittest.TestCase):
    def run_fixture(self, mismatch=""):
        with tempfile.TemporaryDirectory(prefix="kiln-runner-test-") as directory:
            root = Path(directory)
            (root / "probes").mkdir()
            (root / "probes/fixture.mjs").write_text("// fixture\n")
            (root / "fixture.bundle.js").write_text(
                "require('fs').appendFileSync(process.env.FIXTURE_NODE_LOG, 'run\\n');\n"
                "process.stdout.write('answer\\n'); process.stderr.write('warning\\n'); process.exitCode = 7;\n"
            )
            kiln = root / "kiln"
            kiln.write_text("""#!/usr/bin/env python3
import os, pathlib, sys
native = '--native' in sys.argv
with open(os.environ['FIXTURE_KILN_LOG'], 'a') as stream:
    stream.write('native\\n' if native else 'interpreter\\n')
mismatch = os.environ.get('FIXTURE_MISMATCH', '') if native else ''
sys.stdout.write('wrong\\n' if mismatch == 'stdout' else 'answer\\n')
sys.stderr.write('wrong\\n' if mismatch == 'stderr' else 'warning\\n')
sys.exit(8 if mismatch == 'status' else 7)
""")
            kiln.chmod(0o755)
            environment = dict(os.environ, KILN_BIN=str(kiln), KILN_THIRDPARTY_DIR=str(root),
                               KILN_THIRDPARTY_NATIVE="1", KILN_JOBS="1",
                               FIXTURE_NODE_LOG=str(root / "node.calls"),
                               FIXTURE_KILN_LOG=str(root / "kiln.calls"), FIXTURE_MISMATCH=mismatch)
            result = subprocess.run([str(Path(__file__).resolve().with_name("thirdparty.sh"))],
                                    env=environment, capture_output=True, text=True, timeout=20)
            self.assertEqual((root / "node.calls").read_text(), "run\n")
            self.assertEqual((root / "kiln.calls").read_text(), "interpreter\nnative\n")
            return result

    def test_each_engine_runs_once_and_nonzero_status_is_compared(self):
        result = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("thirdparty (native): 1 matched", result.stdout)

    def test_each_stream_and_status_can_independently_fail(self):
        for mismatch in ("stdout", "stderr", "status"):
            with self.subTest(mismatch=mismatch):
                result = self.run_fixture(mismatch)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("1 diverged", result.stdout)


if __name__ == "__main__":
    unittest.main()
