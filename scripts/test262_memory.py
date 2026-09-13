"""Small, dependency-free memory limit and verification helpers for Test262 runners."""
import os
import resource
import signal
import subprocess
import sys
import time


def limit_bytes():
    megabytes = int(os.environ.get("KILN_TEST262_MEMORY_MB", "4096"))
    if megabytes < 64:
        raise ValueError("KILN_TEST262_MEMORY_MB must be at least 64")
    return megabytes * 1024 * 1024


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
