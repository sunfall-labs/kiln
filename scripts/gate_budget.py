"""Divide one gate's CPU/memory allowance among its independent correctness lanes."""
from __future__ import annotations

import os
import subprocess
import sys

from test262_memory import available_cpus


def physical_memory_mb() -> int:
    try:
        return os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE") // (1024 * 1024)
    except (ValueError, OSError):
        try:
            return int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True)) // (1024 * 1024)
        except (OSError, ValueError, subprocess.SubprocessError):
            return 4096


def budget(environment, cpus: int, memory_mb: int, *, quick: bool = False):
    total = min(32, max(1, cpus))
    for name in ("KILN_GATE_JOBS", "KILN_JOBS", "RAYON_NUM_THREADS"):
        if name in environment:
            value = int(environment[name])
            if value < 1:
                raise ValueError(f"{name} must be positive")
            total = min(total, value)
    parallel = total >= (2 if quick else 3) and environment.get("KILN_GATE_SERIAL") != "1"
    if parallel:
        tests = min(8, max(1, total // 4))
        native = min(16, total - tests) if quick else min(8, max(1, total // 4))
        # A full gate prepares native-release benchmark binaries while correctness lanes run.
        # Reserve one worker for that compiler process; its execution remains behind a gate
        # barrier and starts only after these lanes join. At three CPUs there is no spare worker,
        # so the gate leaves preparation until after correctness instead.
        benchmark_worker = 1 if not quick and total >= 4 else 0
        test262 = max(1, total - tests - native - benchmark_worker)
    else:
        tests, native, test262 = min(8, total), min(16, total), total
    # Worker count bounds overlapping cases, not the memory needed by one case. In particular,
    # the extended generated-eval tests exceed 1 GiB even with one worker. Retain the standalone
    # runner's 4 GiB allowance at small CPU budgets; still cap it at half physical memory/16 GiB.
    memory = int(environment.get("KILN_TEST262_MEMORY_MB", min(16384, max(64, memory_mb // 2), max(4096, test262 * 1024))))
    if memory < 64:
        raise ValueError("KILN_TEST262_MEMORY_MB must be at least 64")
    test262 = min(test262, max(1, memory // 1024))
    return int(parallel), total, tests, native, test262, memory


if __name__ == "__main__":
    try:
        print(*budget(os.environ, available_cpus(), physical_memory_mb(), quick="--quick" in sys.argv))
    except ValueError as exc:
        sys.exit(f"gate budget: {exc}")
