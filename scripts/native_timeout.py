#!/usr/bin/env python3
"""Run one native parity invocation with an owned process-group deadline."""

from __future__ import annotations

import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


TERM_GRACE_SECONDS = 0.5
FINAL_DRAIN_SECONDS = 0.5
FINAL_REAP_SECONDS = 0.5
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
        term_started = time.monotonic()
        _signal_group(child.pid, signal.SIGTERM)
        output = None
        try:
            output, _ = child.communicate(timeout=TERM_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            pass

        # communicate() can finish when the leader exits and descendants close their inherited
        # pipes. Keep the group alive for the full TERM grace, then KILL every remaining member
        # of this invocation's isolated group, whether or not the leader has already exited.
        remaining_grace = TERM_GRACE_SECONDS - (time.monotonic() - term_started)
        if remaining_grace > 0:
            time.sleep(remaining_grace)
        _signal_group(child.pid, signal.SIGKILL)

        drain_error = False
        if output is None:
            try:
                output, _ = child.communicate(timeout=FINAL_DRAIN_SECONDS)
            except subprocess.TimeoutExpired as error:
                # An escaped descendant may still own the pipe. Preserve bytes collected so far,
                # close our read end, and reap the owned leader with a bounded wait. Never signal
                # a process group that the child deliberately escaped into.
                output = error.output or b""
                drain_error = True
                if child.stdout is not None:
                    child.stdout.close()
                _signal_group(child.pid, signal.SIGKILL)
                try:
                    child.wait(timeout=FINAL_REAP_SECONDS)
                except subprocess.TimeoutExpired:
                    pass
        if output:
            sys.stdout.buffer.write(output)
            sys.stdout.buffer.flush()
        elapsed = timeout
        print(
            f"KILN_NATIVE_TIMEOUT: invocation exceeded {elapsed:g}s; "
            "its process group was terminated"
            + ("; output drain was bounded and closed" if drain_error else ""),
            file=sys.stderr,
        )
        return TIMEOUT_EXIT

    if output:
        sys.stdout.buffer.write(output)
        sys.stdout.buffer.flush()
    status = child.returncode
    return 128 - status if status < 0 else status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
