import importlib.util
from pathlib import Path
import sys
import unittest

spec = importlib.util.spec_from_file_location("process_measurement", Path(__file__).with_name("process_measurement.py"))
measurement = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measurement)


class ProcessMeasurementTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform in ("linux", "darwin"), "resource RSS units validated on Linux and Darwin")
    def test_large_previous_child_does_not_inflate_small_child(self):
        large = measurement.measure_process([sys.executable, "-c", "x=bytearray(64*1024*1024); print(len(x))"])
        small = measurement.measure_process([sys.executable, "-c", "print('small')"])
        self.assertEqual(large[2], 0)
        self.assertEqual(small[2], 0)
        self.assertGreater(large[4], small[4] + 32 * 1024)

    def test_raw_stdout_and_exit_status_survive_worker_transport(self):
        result = measurement.measure_process([sys.executable, "-c", "import os; os.write(1,b'\\x00\\xff'); raise SystemExit(7)"])
        self.assertEqual(result[1], b"\x00\xff")
        self.assertEqual(result[2], 7)

    def test_timeout_is_not_a_time_or_success(self):
        result = measurement.measure_process([sys.executable, "-c", "import time; time.sleep(5)"], timeout=0.05)
        self.assertIsNone(result[0])
        self.assertEqual(result[2], -1)
        self.assertEqual(result[3], "timeout")


if __name__ == "__main__":
    unittest.main()
