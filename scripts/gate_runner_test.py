"""Resource bounds and real stream/exit-status comparisons for the fast gate."""
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import time
import unittest

from gate_budget import budget
from gate_results import aggregate, binary_snapshot, code_snapshot


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

    def test_full_parallel_gate_reserves_one_worker_for_benchmark_preparation(self):
        parallel, total, tests, native, test262, _ = budget({}, 16, 32768)
        self.assertEqual((parallel, total, tests, native, test262), (1, 16, 4, 4, 7))
        self.assertEqual(sum((tests, native, test262, 1)), total)

        # A three-CPU gate has no spare worker; it must prepare after the correctness lanes.
        parallel, total, tests, native, test262, _ = budget({}, 3, 8192)
        self.assertEqual((parallel, total, tests, native, test262), (1, 3, 1, 1, 1))

    def test_aggregate_rejects_failed_background_process_envelope(self):
        with tempfile.TemporaryDirectory(prefix="kiln-gate-aggregate-") as directory:
            root = Path(directory)
            log = root / "thirdparty.log"
            log.write_text("compile failed\n")
            envelope = {
                "schema": 1,
                "verdict": "failed",
                "source": {
                    "kind": "process",
                    "name": "thirdparty baseline",
                    "code_snapshot_sha256": code_snapshot(),
                    "binary_sha256": binary_snapshot(),
                },
                "measurement": "process",
                "evidence": {"log": str(log)},
            }
            (root / "thirdparty.envelope.json").write_text(json.dumps(envelope))
            result = aggregate(str(root), ["thirdparty baseline"], str(root / "aggregate.json"))
            self.assertEqual(result["verdict"], "failed")
            self.assertEqual(result["failed_steps"], ["thirdparty baseline"])


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
if 'build' in sys.argv:
    output = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
    output.write_text('''#!/usr/bin/env python3\nimport os\nwith open(os.environ["FIXTURE_BINARY_LOG"], "a") as stream: stream.write("run\\\\n")\n''')
    output.chmod(0o755)
    sys.exit(0)
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

    def test_benchmark_waits_for_gate_release_before_any_warmup_or_timing(self):
        with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-barrier-") as directory:
            root = Path(directory)
            (root / "probes").mkdir()
            (root / "probes/fixture.mjs").write_text("// fixture\n")
            (root / "fixture.bundle.js").write_text("// fixture bundle\n")
            kiln = root / "kiln"
            kiln.write_text("""#!/usr/bin/env python3
import os, pathlib, sys
output = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
output.write_text('''#!/usr/bin/env python3\nimport os\nwith open(os.environ["FIXTURE_BINARY_LOG"], "a") as stream: stream.write("run\\\\n")\n''')
output.chmod(0o755)
""")
            kiln.chmod(0o755)
            marker = root / "release"
            binary_log = root / "binary.calls"
            environment = dict(os.environ, KILN_BIN=str(kiln), KILN_THIRDPARTY_DIR=str(root),
                               KILN_THIRDPARTY_NATIVE="0", KILN_JOBS="1",
                               KILN_THIRDPARTY_BUILD_JOBS="1", FIXTURE_BINARY_LOG=str(binary_log))
            process = subprocess.Popen(
                [str(Path(__file__).resolve().with_name("thirdparty.sh")), "--bench",
                 "--bundles", "fixture", "--passes", "1", "--warmup", "0",
                 "--wait-for-release", str(marker)],
                env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                text=True, bufsize=1,
            )
            lines = queue.Queue()

            def read_output():
                assert process.stdout is not None
                for line in process.stdout:
                    lines.put(line)
                lines.put(None)

            reader = threading.Thread(target=read_output, daemon=True)
            reader.start()
            output = []
            reached_barrier = False
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                try:
                    line = lines.get(timeout=0.2)
                except queue.Empty:
                    continue
                if line is None:
                    break
                output.append(line)
                if "waiting for gate release marker" in line:
                    reached_barrier = True
                    break
            self.assertTrue(reached_barrier, "\n".join(output))
            self.assertFalse(binary_log.exists(), "binary ran before the correctness-lane release")
            temporary_marker = marker.with_suffix(".tmp")
            temporary_marker.write_text("measure\n")
            temporary_marker.replace(marker)
            status = process.wait(timeout=30)
            reader.join(timeout=5)
            assert process.stdout is not None
            process.stdout.close()
            while True:
                try:
                    line = lines.get_nowait()
                except queue.Empty:
                    break
                if line is None:
                    break
                output.append(line)
            self.assertEqual(status, 0, "".join(output))
            self.assertEqual(binary_log.read_text(), "run\n" * 3)
            self.assertIn("gate barrier wait completed", "".join(output))
            self.assertIn("serial warmup/measurement phase completed", "".join(output))


if __name__ == "__main__":
    unittest.main()
