"""Validation shared by the benchmark driver and its lightweight unit tests."""

import hashlib


def _describe(value):
    if isinstance(value, bytes):
        return f"<{len(value)} bytes sha256={hashlib.sha256(value).hexdigest()}>"
    return repr(value)


def validate_repeated_checksums(checksums, expected_repetitions=None):
    """Return runtimes to exclude and diagnostics for checksum samples by runtime.

    A stable Node sample is the reference. Every repetition from every runtime must agree with
    it; a runtime whose own output varies is therefore excluded even if its first sample matched.
    If Node itself varies (or is absent), there is no trustworthy reference and all runtimes are
    excluded.
    """
    excluded = set()
    diagnostics = []
    node_values = checksums.get("node", [])
    if not node_values:
        diagnostics.append("node: no checksum reference; excluding all runtimes")
        return set(checksums), diagnostics

    if expected_repetitions is not None and len(node_values) != expected_repetitions:
        diagnostics.append(
            f"node: got {len(node_values)} checksum samples, expected {expected_repetitions}; "
            "excluding all runtimes"
        )
        return set(checksums), diagnostics

    expected = node_values[0]
    for repetition, value in enumerate(node_values[1:], start=2):
        if value != expected:
            excluded.add("node")
            diagnostics.append(
                f"node pass {repetition}: checksum {_describe(value)} != "
                f"node pass 1 {_describe(expected)}"
            )
    if "node" in excluded:
        for runtime in checksums:
            if runtime != "node":
                excluded.add(runtime)
                diagnostics.append(
                    f"{runtime}: no stable node checksum reference; excluding runtime"
                )
        return excluded, diagnostics

    for runtime, values in checksums.items():
        if runtime == "node":
            continue
        if expected_repetitions is not None and len(values) != expected_repetitions:
            excluded.add(runtime)
            diagnostics.append(
                f"{runtime}: got {len(values)} checksum samples, "
                f"expected {expected_repetitions}; excluding runtime"
            )
        for repetition, value in enumerate(values, start=1):
            if value != expected:
                excluded.add(runtime)
                diagnostics.append(
                    f"{runtime} pass {repetition}: checksum {_describe(value)} != "
                    f"node's {_describe(expected)}"
                )
    return excluded, diagnostics
