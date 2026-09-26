"""Small, dependency-free memory limit and verification helpers for Test262 runners."""
import os
import resource
import signal
import subprocess
import sys
import time


MEBIBYTE = 1024 * 1024
GIBIBYTE = 1024 * MEBIBYTE
MEMORY_PER_WORKER = GIBIBYTE


def limit_bytes(environment=None):
    environment = os.environ if environment is None else environment
    megabytes = int(environment.get("KILN_TEST262_MEMORY_MB", "4096"))
    if megabytes < 64:
        raise ValueError("KILN_TEST262_MEMORY_MB must be at least 64")
    return megabytes * MEBIBYTE


def available_cpus():
    try:
        return max(1, len(os.sched_getaffinity(0)))
    except (AttributeError, OSError):
        return max(1, os.cpu_count() or 1)


def worker_count(environment, cpu_count):
    """Bound concurrent cases by CPUs, explicit job limits, and the memory budget.

    The per-worker allowance is a scheduling policy, not a guarantee about an individual
    test's memory use. The subprocess memory cap remains independently enforced.
    """
    limits = [max(1, cpu_count)]
    for name in ("KILN_JOBS", "RAYON_NUM_THREADS"):
        try:
            requested = int(environment.get(name, ""))
        except ValueError:
            continue
        if requested > 0:
            limits.append(requested)
    limits.append(max(1, limit_bytes(environment) // MEMORY_PER_WORKER))
    return min(limits)


def child_environment(environment, cpu_count=None):
    """Copy the caller's environment and set the worker bound for the Kiln child."""
    child = dict(environment)
    child["RAYON_NUM_THREADS"] = str(
        worker_count(environment, available_cpus() if cpu_count is None else cpu_count)
    )
    return child


def _rlimit(limit):
    # macOS reports a small Python process as having a large address space and rejects a useful
    # RLIMIT_AS value during exec.  The RSS watchdog below is the effective cap there; Linux gets
    # both mechanisms.
    if sys.platform.startswith("linux"):
        resource.setrlimit(resource.RLIMIT_AS, (limit, limit))


def _rss(pid):
    try:
        with open(f"/proc/{pid}/status", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except (FileNotFoundError, OSError):
        pass
    try:
        value = subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True)
        return int(value.strip() or 0) * 1024
    except (OSError, ValueError, subprocess.SubprocessError):
        return 0


def run_limited(argv, **kwargs):
    """Run a child with RLIMIT_AS and an RSS watchdog (macOS has weak RLIMIT_AS semantics)."""
    limit = limit_bytes()
    child = subprocess.Popen(argv, preexec_fn=lambda: _rlimit(limit), **kwargs)
    exceeded = False
    while child.poll() is None:
        if _rss(child.pid) > limit:
            exceeded = True
            child.send_signal(signal.SIGTERM)
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
            break
        time.sleep(0.02)
    return child, exceeded


def verify_effective_cap():
    """Prove the mechanism rejects a controlled oversized allocation before the real run."""
    script = "try:\n bytearray(2048 * 1024 * 1024)\nexcept MemoryError:\n raise SystemExit(42)"
    env = os.environ.copy()
    env["KILN_TEST262_MEMORY_MB"] = "512"
    old = os.environ.get("KILN_TEST262_MEMORY_MB")
    os.environ["KILN_TEST262_MEMORY_MB"] = "512"
    try:
        child, exceeded = run_limited([sys.executable, "-c", script],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                     env=env)
    finally:
        if old is None:
            os.environ.pop("KILN_TEST262_MEMORY_MB", None)
        else:
            os.environ["KILN_TEST262_MEMORY_MB"] = old
    if not exceeded and child.returncode != 42:
        raise RuntimeError(
            "memory cap verification did not observe a capped allocation "
            f"(exit={child.returncode})"
        )
