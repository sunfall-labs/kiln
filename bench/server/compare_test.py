import importlib.util
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("compare", HERE / "compare.py")
compare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare)


class CompareHarnessTests(unittest.TestCase):
    def test_paired_variants_alternate_order_each_pass(self):
        baseline = ("baseline", {"hello": Path("baseline")})
        candidate = ("candidate", {"hello": Path("candidate")})
        variants = [baseline, candidate]
        self.assertEqual(compare.paired_variant_order(variants, 1), [baseline, candidate])
        self.assertEqual(compare.paired_variant_order(variants, 2), [candidate, baseline])
        self.assertEqual(compare.paired_variant_order(variants, 3), [baseline, candidate])
        self.assertEqual(compare.runtime_variant_order(variants, "node", 2), [candidate])

    def test_clients_are_started_concurrently(self):
        with tempfile.TemporaryDirectory() as directory:
            fake = Path(directory) / "fake_client.py"
            fake.write_text(
                "import json, time\n"
                "time.sleep(.2)\n"
                "print(json.dumps({'requests': 1, 'errors': 0, 'wrong': 0, 'rps': 1, 'p50_ms': 1, 'p99_ms': 1}))\n"
            )
            started = time.monotonic()
            reports = compare.run_clients(fake, 1, 1, 1, "hello", 3, 2, sys.executable)
            elapsed = time.monotonic() - started
            self.assertEqual(len(reports), 3)
            self.assertLess(elapsed, .5)

    def test_kiln_version_probe_does_not_execute_server_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "launched"
            binary = Path(directory) / "server"
            binary.write_text(f"#!{sys.executable}\nfrom pathlib import Path\nPath({str(marker)!r}).write_text('launched')\n")
            binary.chmod(0o755)
            self.assertEqual(compare.runtime_version("kiln", {"hello": binary}), "not-probed (server binary)")
            self.assertFalse(marker.exists())

    def test_malformed_first_client_cleans_all_and_keeps_raw_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            client = root / "client.py"
            client.write_text("""import os, pathlib, time
root=pathlib.Path(os.environ['CLIENT_ROOT']); (root / str(os.getpid())).write_text('alive')
try:
  fd=os.open(root / 'first', os.O_CREAT|os.O_EXCL|os.O_WRONLY); os.close(fd); print('bad')
except FileExistsError:
  time.sleep(30)
""")
            output = root / "raw"
            output.mkdir()
            old = os.environ.get("CLIENT_ROOT")
            os.environ["CLIENT_ROOT"] = str(root / "pids")
            (root / "pids").mkdir()
            try:
                with self.assertRaises(RuntimeError):
                    compare.run_clients(client, 1, 1, 1, "hello", 3, 2, sys.executable, output)
            finally:
                if old is None: os.environ.pop("CLIENT_ROOT", None)
                else: os.environ["CLIENT_ROOT"] = old
            self.assertEqual(len(list((root / "pids").glob("[0-9]*"))), 3)
            for pid_file in (root / "pids").glob("[0-9]*"):
                pid = int(pid_file.name)
                with self.assertRaises(ProcessLookupError): os.kill(pid, 0)
            self.assertEqual(len(list(output.glob("*.stdout"))), 3)
            self.assertEqual(len(list(output.glob("*.stderr"))), 3)

    def test_stop_process_kills_child_after_leader_exits(self):
        code = "import os, sys, time\nchild=os.fork()\nif child == 0: time.sleep(30)\nelse: print(child, flush=True); os._exit(0)\n"
        process = __import__("subprocess").Popen([sys.executable, "-c", code], stdout=__import__("subprocess").PIPE,
                                                  start_new_session=True)
        child = int(process.stdout.readline())
        process.wait(timeout=2)
        process.stdout.close()
        compare.stop_process(process)
        for _ in range(20):
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                return
            time.sleep(.02)
        self.fail("owned child survived process-group cleanup")


if __name__ == "__main__":
    unittest.main()
