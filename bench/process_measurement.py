"""Measure one program in a fresh worker so child RSS excludes earlier builds/runs."""
import base64
import json
from pathlib import Path
import subprocess
import sys
import time


def measure_process(argv, timeout=120):
    """Return wall ms, raw stdout, exit code, stderr tail and this child's peak KiB.

    Worker startup and result transport are outside the reported program time.
    A fresh worker has exactly one child, making RUSAGE_CHILDREN's high-water
    mark attributable to this execution rather than the benchmark's compilers.
    """
    worker = subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), str(timeout), json.dumps(argv)],
        capture_output=True, timeout=timeout + 10,
    )
    if worker.returncode:
        raise RuntimeError("process measurement worker failed: " +
                           worker.stderr.decode("utf-8", errors="replace"))
    result = json.loads(worker.stdout)
    return (result["wall_ms"], base64.b64decode(result["stdout"]),
            result["exit_code"], result["stderr_tail"], result["peak_rss_kb"])


def _worker(argv, timeout):
    started = time.perf_counter()
    try:
        done = subprocess.run(argv, capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"wall_ms": None, "stdout": "", "exit_code": -1,
                "stderr_tail": "timeout", "peak_rss_kb": None}
    wall_ms = (time.perf_counter() - started) * 1000
    try:
        import resource
        peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        # Darwin reports bytes; Linux and the BSD resource interface report KiB.
        peak_rss_kb = peak / 1024 if sys.platform == "darwin" else peak
    except ImportError:
        peak_rss_kb = None
    stderr = done.stderr.decode("utf-8", errors="replace").strip().splitlines()
    return {"wall_ms": wall_ms, "stdout": base64.b64encode(done.stdout).decode("ascii"),
            "exit_code": done.returncode, "stderr_tail": "\n".join(stderr[-3:]),
            "peak_rss_kb": peak_rss_kb}


if __name__ == "__main__":
    print(json.dumps(_worker(json.loads(sys.argv[2]), float(sys.argv[1]))))
