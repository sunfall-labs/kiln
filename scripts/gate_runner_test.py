"""Resource bounds and real stream/exit-status comparisons for the fast gate."""
import hashlib
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

    def benchmark_fixture(self, root, bundle_names, baseline_path, *, build_fail=False,
                          runtime_exit=0, node_exit=0, runtime_stdout="oracle\n", runtime_stderr="",
                          runtime_sleep=0, update=False, create_bundles=True,
                          profile="native-release", node_env=None, echo_profile_env=False,
                          bundle_source=None):
        (root / "probes").mkdir(exist_ok=True)
        if create_bundles:
            for name in bundle_names:
                node_exit_statement = f"process.exitCode = {node_exit};\n" if node_exit else ""
                (root / f"{name}.bundle.js").write_text(
                    (bundle_source or "process.stdout.write('oracle\\n');\n") + node_exit_statement
                )
        kiln = root / "kiln"
        kiln.write_text("""#!/usr/bin/env python3
import os, pathlib, sys
if 'build' in sys.argv:
    pathlib.Path(os.environ['FIXTURE_BUILD_ENV_LOG']).write_text(
        __import__('json').dumps({'node_env': os.environ.get('NODE_ENV'), 'tz': os.environ.get('TZ')}))
    if os.environ.get('FIXTURE_BUILD_FAIL') == '1':
        print('intentional build failure', file=sys.stderr)
        sys.exit(3)
    output = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
    output.write_text('''#!/usr/bin/env python3
import os, sys
import time
time.sleep(float(os.environ.get('FIXTURE_BINARY_SLEEP', '0')))
if os.environ.get('FIXTURE_ECHO_PROFILE_ENV') == '1':
    sys.stdout.write(str(os.environ.get('NODE_ENV')) + '|' + str(os.environ.get('TZ')))
else:
    sys.stdout.write(os.environ.get('FIXTURE_BINARY_STDOUT', ''))
sys.stderr.write(os.environ.get('FIXTURE_BINARY_STDERR', ''))
sys.exit(int(os.environ.get('FIXTURE_BINARY_EXIT', '0')))
''')
    output.chmod(0o755)
""")
        kiln.chmod(0o755)
        environment = dict(
            os.environ,
            KILN_BIN=str(kiln),
            KILN_THIRDPARTY_DIR=str(root),
            KILN_THIRDPARTY_NATIVE="0",
            KILN_THIRDPARTY_PROFILE=profile,
            KILN_JOBS="1",
            KILN_THIRDPARTY_BUILD_JOBS="1",
            FIXTURE_BUILD_FAIL="1" if build_fail else "0",
            FIXTURE_BINARY_EXIT=str(runtime_exit),
            FIXTURE_BINARY_STDOUT=runtime_stdout,
            FIXTURE_BINARY_STDERR=runtime_stderr,
            FIXTURE_BINARY_SLEEP=str(runtime_sleep),
            FIXTURE_ECHO_PROFILE_ENV="1" if echo_profile_env else "0",
            FIXTURE_BUILD_ENV_LOG=str(root / "build-env.json"),
        )
        environment.pop("NODE_ENV", None)
        if node_env is not None:
            environment["NODE_ENV"] = node_env
        command = [
            str(Path(__file__).resolve().with_name("thirdparty.sh")), "--bench",
            "--bundles", ",".join(bundle_names), "--passes", "1", "--warmup", "0",
            "--baseline-file", str(baseline_path),
        ]
        if update:
            command.append("--update")
        return subprocess.run(command, env=environment, capture_output=True, text=True, timeout=30)

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
pathlib.Path(os.environ['FIXTURE_BUILD_ENV_LOG']).write_text(
    __import__('json').dumps({'tmpdir': os.environ.get('TMPDIR'), 'output': str(output)}))
output.write_text('''#!/usr/bin/env python3\nimport os\nwith open(os.environ["FIXTURE_BINARY_LOG"], "a") as stream: stream.write("run\\\\n")\n''')
output.chmod(0o755)
""")
            kiln.chmod(0o755)
            marker = root / "release"
            binary_log = root / "binary.calls"
            runtime_tmpdir = root / "runtime-tmp"
            stage_dir = root / "stage"
            runtime_tmpdir.mkdir()
            stage_dir.mkdir()
            baseline_path = root / "baseline.json"
            build_env_log = root / "build-env.json"
            environment = dict(os.environ, KILN_BIN=str(kiln), KILN_THIRDPARTY_DIR=str(root),
                               KILN_THIRDPARTY_NATIVE="0", KILN_JOBS="1",
                               KILN_THIRDPARTY_BUILD_JOBS="1", FIXTURE_BINARY_LOG=str(binary_log),
                               TMPDIR=str(runtime_tmpdir),
                               KILN_THIRDPARTY_WORK_TMPDIR=str(stage_dir),
                               FIXTURE_BUILD_ENV_LOG=str(build_env_log))
            process = subprocess.Popen(
                [str(Path(__file__).resolve().with_name("thirdparty.sh")), "--bench",
                 "--bundles", "fixture", "--passes", "1", "--warmup", "0", "--update",
                 "--baseline-file", str(baseline_path),
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
            build_environment = json.loads(build_env_log.read_text())
            self.assertEqual(build_environment["tmpdir"], str(runtime_tmpdir))
            Path(build_environment["output"]).relative_to(stage_dir)
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
            # One Node-oracle and one release-profile validation precede two warmups
            # and one timed pass.
            self.assertEqual(binary_log.read_text(), "run\n" * 4)
            self.assertIn("gate barrier wait completed", "".join(output))
            self.assertIn("serial warmup/measurement phase completed", "".join(output))
            measured_baseline = json.loads(baseline_path.read_text())
            self.assertEqual(measured_baseline["schema"], 2)
            self.assertEqual(len(measured_baseline["platforms"]), 1)
            platform_key, platform_record = next(iter(measured_baseline["platforms"].items()))
            self.assertIn("native-release", platform_key)
            self.assertEqual(platform_record["profile"], "native-release")
            self.assertTrue(platform_record["platform"]["system"])
            self.assertTrue(platform_record["platform"]["machine"])
            self.assertTrue(platform_record["platform"]["cpu_model"])
            self.assertTrue(platform_record["provenance"]["source_commit"])
            self.assertTrue(platform_record["provenance"]["rustc_version"])
            self.assertTrue(platform_record["provenance"]["clang_version"])
            self.assertEqual(
                    platform_record["bundles"]["fixture"]["workload_sha256"],
                    hashlib.sha256((root / "fixture.bundle.js").read_bytes()).hexdigest(),
            )
            self.assertEqual(platform_record["bundles"]["fixture"]["measurement_provenance"]["profile"],
                             "native-release")
            self.assertEqual(platform_record["provenance"]["scope"], "latest_update")

    def test_benchmark_rejects_baseline_from_another_platform_before_building(self):
        with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-platform-") as directory:
            root = Path(directory)
            (root / "fixture.bundle.js").write_text("// fixture bundle\n")
            kiln_log = root / "kiln.calls"
            kiln = root / "kiln"
            kiln.write_text("#!/usr/bin/env python3\nimport os\nopen(os.environ['FIXTURE_KILN_LOG'], 'a').write('build\\n')\n")
            kiln.chmod(0o755)
            baseline_path = root / "baseline.json"
            baseline_path.write_text(json.dumps({
                "schema": 2,
                "threshold_pct": 25,
                "platforms": {
                    "macos-arm64-apple-m5-pro-native-release": {
                        "platform": {"system": "Darwin", "machine": "arm64", "cpu_model": "Apple M5 Pro"},
                        "profile": "native-release",
                        "provenance": {"status": "inferred"},
                        "bundles": {"fixture": {"median_ms": 1, "min_ms": 1, "workload_sha256": "unused"}},
                    }
                },
            }))
            environment = dict(os.environ, KILN_BIN=str(kiln), KILN_THIRDPARTY_DIR=str(root),
                               KILN_THIRDPARTY_NATIVE="0", KILN_JOBS="1",
                               FIXTURE_KILN_LOG=str(kiln_log))
            result = subprocess.run(
                [str(Path(__file__).resolve().with_name("thirdparty.sh")), "--bench",
                 "--bundles", "fixture", "--baseline-file", str(baseline_path)],
                env=environment, capture_output=True, text=True, timeout=20,
            )
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Cross-platform timings are incomparable", result.stderr)
            self.assertFalse(kiln_log.exists(), "a mismatched baseline must be rejected before compilation")

    def test_update_refuses_failed_build_run_or_release_output_without_changing_baseline(self):
        with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-update-failure-") as directory:
            root = Path(directory)
            baseline_path = root / "baseline.json"
            original = b'{"schema":2,"platforms":{},"sentinel":"unchanged"}\n'
            for case, options in (
                ("build", {"build_fail": True}),
                ("runtime", {"runtime_exit": 3}),
                ("nonzero-oracle", {"runtime_exit": 3, "node_exit": 3}),
                ("release-output", {"runtime_stdout": "wrong\\n"}),
            ):
                with self.subTest(case=case):
                    baseline_path.write_bytes(original)
                    result = self.benchmark_fixture(root, ["fixture"], baseline_path,
                                                    update=True, **options)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn("refusing to update thirdparty baseline", result.stderr)
                    if case == "nonzero-oracle":
                        self.assertIn("Node oracle exited 3", result.stderr)
                    if case == "release-output":
                        self.assertIn("native-release output differs from Node oracle", result.stderr)
                    self.assertEqual(baseline_path.read_bytes(), original)

    def test_empty_normal_comparison_is_rejected_and_partial_update_provenance_is_scoped(self):
        with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-update-scope-") as directory:
            root = Path(directory)
            baseline_path = root / "baseline.json"
            initial = self.benchmark_fixture(root, ["first", "second"], baseline_path, update=True)
            self.assertEqual(initial.returncode, 0, initial.stdout + initial.stderr)
            baseline = json.loads(baseline_path.read_text())
            _, record = next(iter(baseline["platforms"].items()))
            old_second_provenance = record["bundles"]["second"]["measurement_provenance"]

            partial = self.benchmark_fixture(root, ["first"], baseline_path, update=True)
            self.assertEqual(partial.returncode, 0, partial.stdout + partial.stderr)
            baseline = json.loads(baseline_path.read_text())
            _, record = next(iter(baseline["platforms"].items()))
            self.assertEqual(record["provenance"]["scope"], "latest_update")
            self.assertEqual(record["provenance"]["updated_bundles"], ["first"])
            self.assertEqual(record["bundles"]["second"]["measurement_provenance"], old_second_provenance)
            self.assertEqual(record["bundles"]["first"]["measurement_provenance"]["passes"], 1)

            (root / "first.bundle.js").unlink()
            (root / "second.bundle.js").unlink()
            before = baseline_path.read_bytes()
            empty_update = self.benchmark_fixture(root, ["first", "second"], baseline_path,
                                                  update=True, create_bundles=False)
            self.assertNotEqual(empty_update.returncode, 0, empty_update.stdout + empty_update.stderr)
            self.assertIn("none are present", empty_update.stderr)
            self.assertEqual(baseline_path.read_bytes(), before)

            empty = self.benchmark_fixture(root, ["first", "second"], baseline_path,
                                           create_bundles=False)
            self.assertNotEqual(empty.returncode, 0, empty.stdout + empty.stderr)
            self.assertIn("none are present", empty.stderr)
            self.assertEqual(baseline_path.read_bytes(), before)

    def test_deliberate_update_accepts_a_timing_regression(self):
        with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-update-regression-") as directory:
            root = Path(directory)
            baseline_path = root / "baseline.json"
            initial = self.benchmark_fixture(root, ["fixture"], baseline_path, update=True)
            self.assertEqual(initial.returncode, 0, initial.stdout + initial.stderr)
            baseline = json.loads(baseline_path.read_text())
            _, record = next(iter(baseline["platforms"].items()))
            record["bundles"]["fixture"]["median_ms"] = 0.01
            baseline_path.write_text(json.dumps(baseline))

            update = self.benchmark_fixture(root, ["fixture"], baseline_path,
                                            runtime_sleep=0.02, update=True)
            self.assertEqual(update.returncode, 0, update.stdout + update.stderr)
            self.assertIn("REGRESSED", update.stdout)
            refreshed = json.loads(baseline_path.read_text())
            _, refreshed_record = next(iter(refreshed["platforms"].items()))
            self.assertGreater(refreshed_record["bundles"]["fixture"]["median_ms"], 0.01)

    def test_profile_environment_matches_build_oracle_and_every_native_run(self):
        bundle_source = "process.stdout.write(String(process.env.NODE_ENV)+'|'+String(process.env.TZ));\n"
        for profile, inherited_node_env, expected_node_env in (
            ("native-release", None, "production"),
            ("native-release", "development", "production"),
            ("native-dev", "development", "development"),
        ):
            with self.subTest(profile=profile, inherited_node_env=inherited_node_env):
                with tempfile.TemporaryDirectory(prefix="kiln-thirdparty-profile-env-") as directory:
                    root = Path(directory)
                    result = self.benchmark_fixture(
                        root, ["fixture"], root / "baseline.json", update=True,
                        profile=profile, node_env=inherited_node_env,
                        echo_profile_env=True, bundle_source=bundle_source,
                    )
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    build_environment = json.loads((root / "build-env.json").read_text())
                    self.assertEqual(build_environment,
                                     {"node_env": expected_node_env, "tz": "UTC"})


if __name__ == "__main__":
    unittest.main()
