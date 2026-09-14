import importlib.util
import os
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
