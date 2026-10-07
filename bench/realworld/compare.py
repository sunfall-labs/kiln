#!/usr/bin/env python3
"""Real-world TypeScript programs under every runtime, in one table.

Rules (same as bench/compare.sh, extended to AOT competitors and servers):

- node's stdout is the checksum. Any runtime whose stdout differs did different
  work: reported as a divergence, never as a time.
- A missing toolchain, a refused build, or a capability a runtime does not have
  (porffor/hermes have no node:fs/node:http) is "unsupported", not a failure.
- CLI rows report wall time (median of PASSES), peak child RSS, and artifact
  size for AOT outputs. Startup is inside the number on purpose: real CLIs are
  startup-dominated, which is exactly what this suite measures.
- The server row reports requests/s plus body verification from drive-server.mjs.
"""
import hashlib
import json
import os
import select
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time

PASSES = int(os.environ.get("PASSES", "3"))
SERVER_REQUESTS = os.environ.get("SERVER_REQUESTS", "2000")
KILN = os.environ.get("KILN_BIN", "target/release/kiln")
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, ROOT)
from bench.checksum_validation import validate_repeated_checksums
from bench.process_measurement import measure_process

_TOOLCHAIN_BIN = os.path.join(
    ROOT, "bench", "realworld", ".toolchains", "node_modules", ".bin"
)
PORF = os.environ.get("PORF_BIN", "") or shutil.which("porf") or ""
if not (PORF and os.path.exists(PORF)):
    PORF = os.path.join(_TOOLCHAIN_BIN, "porf")
if not (PORF and os.path.exists(PORF)):
    PORF = ""
HERMESC = os.environ.get("HERMESC_BIN", shutil.which("hermesc") or "")
HVM = os.environ.get("HVM_BIN", shutil.which("hvm") or shutil.which("hermes") or "")
ESBUILD = (
    os.environ.get("ESBUILD_BIN", "")
    or shutil.which("esbuild")
    or os.path.join(_TOOLCHAIN_BIN, "esbuild")
)
if not (ESBUILD and os.path.exists(ESBUILD)):
    ESBUILD = ""
PROGRAMS = os.path.join(ROOT, "bench", "realworld", "programs")
FIXTURE = os.path.join(ROOT, "bench", "realworld", "inputs", "users.json")
DRIVER = os.path.join(ROOT, "bench", "realworld", "drive-server.mjs")

CLI = [
    # name, file, tier, needs_node, argv
    ("json-transform", "json-transform.ts", "A", False, []),
    ("template-render", "template-render.ts", "A", False, []),
    ("file-cli", "file-cli.ts", "B", True, [FIXTURE]),
]
SERVER = ("http-server", "http-server.ts")


def run(argv, timeout=120):
    """Run one process; return (wall_ms, stdout_bytes, returncode, stderr_tail)."""
    started = time.perf_counter()
    try:
        done = subprocess.run(argv, capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return None, b"", -1, "timeout"
    wall = (time.perf_counter() - started) * 1000.0
    err = done.stderr.decode("utf-8", errors="replace").strip().splitlines()
    return wall, done.stdout, done.returncode, "\n".join(err[-3:])


def artifact_bytes(path):
    if path and os.path.exists(path):
        return os.path.getsize(path)
    return None


def build_kiln(src, needs_node, work, tag):
    out = os.path.join(work, f"kiln-{tag}")
    argv = [KILN, "--one-shot"]
    if needs_node:
        argv.append("--node")
    argv += ["build", src, "--profile", "native-release", "--output", out]
    wall, _, code, err = run(argv, timeout=600)
    if code == 0 and os.path.exists(out):
        return out, None
    return None, f"kiln build refused (exit {code} {err[:120]})"


def strip_types(src, work, tag):
    """Type-strip a .ts program to plain JS for engines without TS syntax.

    Returns (js_path, note): js_path is src unchanged when no stripping is
    needed or available engines are missing (caller reports unsupported).
    """
    if not src.endswith(".ts"):
        return src, ""
    if not ESBUILD:
        return src, "esbuild is not available for type-stripping"
    js = os.path.join(work, f"{tag}.js")
    _, _, code, err = run([ESBUILD, src, "--outfile=" + js], timeout=120)
    if code != 0:
        return src, f"esbuild strip failed (exit {code} {err[:120]})"
    return js, ""


def build_porffor(src, work, tag):
    if not PORF:
        return None, "porf not installed (run fetch-competitors.sh)"
    src, note = strip_types(src, work, tag)
    if note:
        return None, f"porffor needs type-stripped JS: {note}"
    out = os.path.join(work, f"porf-{tag}")
    wall, _, code, err = run([PORF, src, "-o", out], timeout=600)
    if code == 0 and os.path.exists(out):
        return out, None
    return None, f"porffor refused (exit {code} {err[:120]})"


def build_hermes(src, work, tag):
    if not (HERMESC and HVM):
        return None, "hermesc/hvm not on PATH (run fetch-competitors.sh)"
    src, note = strip_types(src, work, tag)
    if note:
        return None, f"hermes needs type-stripped JS: {note}"
    bundle = os.path.join(work, f"{tag}.hbc")
    wall, _, code, err = run(
        [HERMESC, "-emit-binary", "-out", bundle, src], timeout=300
    )
    if code != 0:
        return None, f"hermesc refused (exit {code} {err[:120]})"
    return bundle, None


def measure_cli(name, src, needs_node, argv, work):
    """Run one CLI program under every runtime. Returns (row, notes)."""
    kiln_flags = ["--node"] if needs_node else []
    commands = {
        "node": ["node", src, *argv],
        "bun": ["bun", "run", src, *argv],
        "kiln": [KILN, "--one-shot", *kiln_flags, "run", src, *argv],
    }
    artifacts = {}
    unsupported = {}

    # Both tiers use the same native-release AOT path. `--node` provisions the
    # filesystem adapter for Tier B while keeping the executable standalone.
    native, note = build_kiln(src, needs_node, work, name)
    if native:
        commands["kiln-native"] = [native, *argv]
        artifacts["kiln-native"] = artifact_bytes(native)
    else:
        unsupported["kiln-native"] = note

    if name in ("json-transform", "template-render"):  # tier A: portable subset
        porf_bin, note = build_porffor(src, work, name)
        if porf_bin:
            commands["porffor"] = [porf_bin]
            artifacts["porffor"] = artifact_bytes(porf_bin)
        else:
            unsupported["porffor"] = note
        hbc, note = build_hermes(src, work, name)
        if hbc:
            commands["hermes"] = [HVM, hbc]
            artifacts["hermes"] = artifact_bytes(hbc)
        else:
            unsupported["hermes"] = note
    else:
        unsupported["porffor"] = "no node:fs/node:http capability (tier B)"
        unsupported["hermes"] = "no node:fs/node:http capability (tier B)"

    measured, rss, checksums, divergences = {}, {}, {}, []
    for runtime, argv in commands.items():
        samples, outputs, peaks = [], [], []
        for _ in range(PASSES):
            wall, stdout, code, err, peak = measure_process(argv)
            if wall is None or code != 0:
                divergences.append(f"{name} {runtime}: exit {code} {err[:100]}")
                samples = []
                break
            samples.append(wall)
            outputs.append(stdout)
            if peak is not None:
                peaks.append(peak)
        if samples:
            measured[runtime] = statistics.median(samples)
            if peaks:
                rss[runtime] = max(peaks)
            checksums[runtime] = outputs
    excluded, checksum_diagnostics = validate_repeated_checksums(checksums, PASSES)
    divergences.extend(f"{name} {diagnostic}" for diagnostic in checksum_diagnostics)
    for runtime in excluded:
        measured.pop(runtime, None)
    return (
        {
            "program": name,
            "ms": measured,
            "checksum_reference": "node",
            "checksum_sha256": {
                runtime: [hashlib.sha256(output).hexdigest() for output in outputs]
                for runtime, outputs in checksums.items()
            },
            "checksum_excluded": sorted(excluded),
            "peak_rss_kb": rss,
            "peak_rss_scope": "maximum child-lifetime peak across repetitions; excludes builds; includes inherited pre-exec image",
            "artifact_bytes": artifacts,
            "execution_modes": {
                "node": "direct TypeScript",
                "bun": "direct TypeScript",
                "kiln": {"kind": "interpreter", "node_adapter": needs_node},
                "kiln-native": {"kind": "AOT", "profile": "native-release",
                                "node_adapter": needs_node},
            },
        },
        unsupported,
        divergences,
    )


def wait_for_port_closed(port, deadline=10):
    end = time.time() + deadline
    while time.time() < end:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            time.sleep(0.1)
        except OSError:
            return True
    return False


def wait_for_server(server, port, deadline=20):
    """Wait until the server accepts connections or exits, with a deadline.

    Readiness is the open port, not the "listening" line: kiln serves requests
    without ever invoking the listen callback, so gating on the line hangs
    forever there. The first line is still captured for diagnostics. Returns
    (ready, first_line, output_tail). Never blocks past the deadline.
    """
    first_line, tail = "", ""
    end = time.time() + deadline
    while time.time() < end:
        if server.poll() is not None:
            try:
                rest = server.stdout.read() or ""
            except Exception:
                rest = ""
            return False, first_line, (tail + rest)[-300:]
        r, _, _ = select.select([server.stdout], [], [], 0.5)
        if r:
            try:
                chunk = server.stdout.readline()
            except Exception:
                chunk = ""
            if chunk:
                tail += chunk
                if not first_line:
                    first_line = chunk.strip()
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.3).close()
            return True, first_line, tail[-300:]
        except OSError:
            pass
    return False, first_line, tail[-300:]


def measure_server(work):
    """Serve drive-server.mjs from each capable runtime. Returns (row, notes)."""
    name, filename = SERVER
    src = os.path.join(PROGRAMS, filename)
    kiln_bin, note = build_kiln(src, True, work, name)
    fl = ["--node"]
    commands = {
        "node": ["node", src],
        "bun": ["bun", "run", src],
        "kiln": [KILN, "--one-shot", *fl, "run", src],
    }
    unsupported = {}
    if kiln_bin:
        commands["kiln-native"] = [kiln_bin]
    else:
        unsupported["kiln-native"] = note
    unsupported["porffor"] = "no node:http capability (tier B)"
    unsupported["hermes"] = "no node:http capability (tier B)"

    rps, notes, divergences, driver_reports = {}, {}, [], {}
    for runtime, argv in commands.items():
        server = subprocess.Popen(
            argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True
        )
        try:
            ready, first_line, tail = wait_for_server(server, 18091)
            if not ready:
                divergences.append(
                    f"{name} {runtime}: never served "
                    f"(first line {first_line[:80]!r} {tail[-80:]!r})"
                )
                continue
            if first_line != "listening":
                # Served, but the listen callback never fired: a real API gap
                # (see NOTES), not a serving failure. The driver below still
                # verifies every body, so the rps number is trustworthy.
                notes[runtime] = (
                    f"served without invoking the listen callback "
                    f"(first line {first_line[:80]!r})"
                )
            wall, out, code, err = run(["node", DRIVER, SERVER_REQUESTS], timeout=120)
            if code != 0:
                divergences.append(f"{name} {runtime}: driver exit {code} {err[:100]}")
                continue
            try:
                report = json.loads(out.decode().strip().splitlines()[-1])
            except (ValueError, IndexError):
                divergences.append(f"{name} {runtime}: driver output unreadable")
                continue
            driver_reports[runtime] = report
            if report.get("requests") != int(SERVER_REQUESTS) or report.get("wrong") != 0:
                divergences.append(
                    f"{name} {runtime}: incomplete or wrong responses: {report}"
                )
                continue
            rps[runtime] = report["rps"]
        finally:
            server.kill()
            server.wait()
        wait_for_port_closed(18091)
    return {"program": name, "rps": rps,
            "driver_reports": driver_reports,
            "connection_mode": "sequential requests with one shared keep-alive agent",
            "execution_modes": {
                "node": "direct TypeScript",
                "bun": "direct TypeScript",
                "kiln": {"kind": "interpreter", "node_adapter": True},
                "kiln-native": {"kind": "AOT", "profile": "native-release",
                                "node_adapter": True},
            }}, unsupported, divergences, notes


def repo_provenance():
    """Record binary identity separately from the checkout running the harness."""
    def run_quiet(argv):
        try:
            done = subprocess.run(argv, capture_output=True, timeout=30)
            return done.stdout.decode().strip()
        except Exception:
            return ""
    runner_rev = run_quiet(["git", "-C", ROOT, "rev-parse", "HEAD"])
    binary_path = os.path.abspath(os.path.join(ROOT, KILN))
    binary_digest = hashlib.sha256()
    with open(binary_path, "rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            binary_digest.update(chunk)
    binary_sha256 = binary_digest.hexdigest()
    with open(__file__, "rb") as runner:
        runner_sha256 = hashlib.sha256(runner.read()).hexdigest()
    try:
        kiln_mtime = int(os.path.getmtime(os.path.join(ROOT, KILN)))
    except OSError:
        kiln_mtime = 0
    versions = {}
    for name, argv in (
        ("node", ["node", "--version"]),
        ("bun", ["bun", "--version"]),
        ("porffor", [PORF, "--version"] if PORF else None),
    ):
        if argv:
            versions[name] = run_quiet(argv)[:40]
    return {
        "kiln_rev": os.environ.get("KILN_SOURCE_REV"),
        "kiln_rev_source": "caller declaration; binary identity recorded independently",
        "kiln_binary": binary_path,
        "kiln_binary_sha256": binary_sha256,
        "runner_rev": runner_rev,
        "runner_sha256": runner_sha256,
        "kiln_bin_mtime": kiln_mtime,
        "versions": versions,
    }


def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else ""
    for tool in ("node", "bun"):
        if not shutil.which(tool):
            print(f"realworld/compare: {tool} is not on PATH", file=sys.stderr)
            sys.exit(1)
    if not (os.path.exists(KILN) and os.access(KILN, os.X_OK)):
        print(f"realworld/compare: no kiln binary at {KILN}", file=sys.stderr)
        sys.exit(1)
    provenance = repo_provenance()
    work = tempfile.mkdtemp(prefix="kiln-realworld.")
    rows, unsupported_all, divergences = [], {}, []
    server_notes = {}
    try:
        for name, src, tier, needs_node, argv in CLI:
            row, unsupported, divs = measure_cli(
                name, os.path.join(PROGRAMS, src), needs_node, argv, work
            )
            row["tier"] = tier
            rows.append(row)
            unsupported_all.update({f"{name} {k}": v for k, v in unsupported.items()})
            divergences += divs
        row, unsupported, divs, server_notes = measure_server(work)
        row["tier"] = "B(server)"
        rows.append(row)
        unsupported_all.update({f"http-server {k}": v for k, v in unsupported.items()})
        divergences += divs
    finally:
        shutil.rmtree(work, ignore_errors=True)

    print("\nCLI: wall ms (median of %d), peak child RSS KB, artifact bytes" % PASSES)
    print(f"{'program':16s} {'node':>9s} {'bun':>9s} {'kiln':>9s} "
          f"{'native':>9s} {'porffor':>9s} {'hermes':>9s}")
    for row in rows:
        if "ms" not in row:
            continue
        cells = []
        for rt in ("node", "bun", "kiln", "kiln-native", "porffor", "hermes"):
            v = row["ms"].get(rt)
            cells.append(f"{v:9.1f}" if v is not None else f"{'--':>9s}")
        print(f"{row['program']:16s} " + " ".join(cells))
    for row in rows:
        if "rps" not in row:
            continue
        cells = " ".join(
            f"{row['rps'].get(rt, '--'):>9}" if isinstance(row['rps'].get(rt), str)
            else (f"{row['rps'][rt]:>9d}" if rt in row["rps"] else f"{'--':>9s}")
            for rt in ("node", "bun", "kiln", "kiln-native")
        )
        print(f"\nserver {row['program']}: requests/s (each runtime serving, node driving)")
        print(f"{'':16s} {'node':>9s} {'bun':>9s} {'kiln':>9s} {'native':>9s}")
        print(f"{'':16s} {cells}")
    if unsupported_all:
        print("\nunsupported (not failures):")
        for key in sorted(unsupported_all):
            print(f"  {key}: {unsupported_all[key]}")
    if server_notes:
        print("\nserver notes (measured anyway; bodies verified):")
        for key in sorted(server_notes):
            print(f"  http-server {key}: {server_notes[key]}")
    if divergences:
        print("\ndivergences (excluded above):", file=sys.stderr)
        for line in divergences:
            print(f"  {line}", file=sys.stderr)
    if out_path:
        with open(out_path, "w") as handle:
            json.dump(
                {"passes": PASSES, "rows": rows, "unsupported": unsupported_all,
                 "server_notes": server_notes, "divergences": divergences,
                 "provenance": provenance},
                handle, indent=2,
            )
            handle.write("\n")
        print(f"\nwrote {out_path}")


if __name__ == "__main__":
    main()
