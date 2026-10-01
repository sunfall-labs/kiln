#!/usr/bin/env python3
"""Run one native parity invocation with an owned process-group deadline."""

from __future__ import annotations

import math
import os
from pathlib import Path
import signal
import subprocess
import sys


TERM_GRACE_SECONDS = 0.5
TIMEOUT_EXIT = 124


def _signal_group(pid: int, sig: int) -> None:
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass


def main(argv: list[str]) -> int:
    if len(argv) < 4 or argv[2] != "--":
        print("usage: native_timeout.py SECONDS MARKER -- COMMAND [ARG ...]", file=sys.stderr)
        return 2
    try:
        timeout = float(argv[0])
    except ValueError:
        print("native_timeout.py: deadline must be a positive number of seconds", file=sys.stderr)
        return 2
    if not math.isfinite(timeout) or timeout <= 0:
        print("native_timeout.py: deadline must be a positive number of seconds", file=sys.stderr)
        return 2

    marker = Path(argv[1])
    command = argv[3:]
    try:
        child = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except OSError as error:
        print(f"native_timeout.py: could not start command: {error}", file=sys.stderr)
        return 127

    try:
        output, _ = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        marker.touch()
        _signal_group(child.pid, signal.SIGTERM)
        try:
            output, _ = child.communicate(timeout=TERM_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            _signal_group(child.pid, signal.SIGKILL)
            output, _ = child.communicate()
        if output:
            sys.stdout.buffer.write(output)
            sys.stdout.buffer.flush()
        elapsed = timeout
        print(
            f"KILN_NATIVE_TIMEOUT: invocation exceeded {elapsed:g}s; "
            "its process group was terminated",
            file=sys.stderr,
        )
        return TIMEOUT_EXIT

    if output:
        sys.stdout.buffer.write(output)
        sys.stdout.buffer.flush()
    return child.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
