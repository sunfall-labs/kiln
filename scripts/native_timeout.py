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


class InvocationInterrupted(Exception):
    def __init__(self, signum: int) -> None:
        self.signum = signum


def _signal_group(pid: int, sig: int) -> None:
    try:
        os.killpg(pid, sig)
    except ProcessLookupError:
        pass


def _terminate_owned_group(child: subprocess.Popen[bytes]) -> tuple[bytes, bool]:
    """TERM, wait a bounded grace, KILL the owned group, and bound pipe draining."""
    term_started = time.monotonic()
    _signal_group(child.pid, signal.SIGTERM)
    output: bytes | None = None
    try:
        output, _ = child.communicate(timeout=TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass

    # communicate() may finish when the leader exits and descendants close inherited pipes.
    # Keep the group alive for the full grace, then kill every remaining owned member.
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
    return output or b"", drain_error


def _stop_handling_interrupts() -> None:
    # A second Ctrl-C/TERM should not interrupt group cleanup after the first was accepted.
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)


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

    def interrupted(signum: int, _frame: object) -> None:
        raise InvocationInterrupted(signum)

    old_sigint = signal.signal(signal.SIGINT, interrupted)
    old_sigterm = signal.signal(signal.SIGTERM, interrupted)
    try:
        output, _ = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        marker.touch()
        _stop_handling_interrupts()
        output, drain_error = _terminate_owned_group(child)
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
    except InvocationInterrupted as interruption:
        _stop_handling_interrupts()
        output, drain_error = _terminate_owned_group(child)
        marker_error = ""
        try:
            marker.with_name(marker.name + ".interrupted").touch()
        except OSError as error:
            # The native runner may remove its scratch directory as it exits. Cleanup owns the
            # child group; recording the interruption is best effort and must never precede it.
            marker_error = f"; could not record interruption marker: {error}"
        if output:
            sys.stdout.buffer.write(output)
            sys.stdout.buffer.flush()
        print(
            f"KILN_NATIVE_INTERRUPTED: received signal {interruption.signum}; "
            "its process group was terminated"
            + ("; output drain was bounded and closed" if drain_error else "")
            + marker_error,
            file=sys.stderr,
        )
        return 128 + interruption.signum
    else:
        signal.signal(signal.SIGINT, old_sigint)
        signal.signal(signal.SIGTERM, old_sigterm)

    if output:
        sys.stdout.buffer.write(output)
        sys.stdout.buffer.flush()
    status = child.returncode
    return 128 - status if status < 0 else status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
