#!/usr/bin/env python3
"""Time one cargo rebuild and check which packages it recompiled.

design.md §12 asks that Rust rebuild time be monitored continuously, and §13 asks CI to record
the clean build, the leaf-edit rebuild and the central-type-edit rebuild.

Wall clock from a shared CI runner is a trend signal at best, so the assertion this script
actually enforces is the machine-independent one: *which* packages cargo decided to recompile.
That is a property of the dependency graph rather than of the runner, and it is what §12 and
docs/adr/0004 are really about - a no-op rebuild must recompile nothing, and a leaf edit must
not drag the foundational crates along with it. The timings are recorded next to it as
information, not as a gate.

Written against the standard library only so it runs on a bare GitHub runner.
"""

import argparse
import json
import pathlib
import subprocess
import sys
import time

# design.md §13, "initial goals on a fixed reference machine". A GitHub-hosted runner is not
# that machine, so these are printed for orientation and are never asserted on here.
REFERENCE_GOAL_MS = {
    "noop": 1_000,
    "leaf-edit": 3_000,
}

# What each probe corresponds to in §13's list of measurements CI should record.
MEASUREMENT = {
    "clean": "Clean Rust build",
    "noop": "No-op compiler rebuild",
    "leaf-edit": "Incremental Rust build after leaf edit",
    "central-edit": "Incremental Rust build after IR edit",
}


def repo_root():
    return pathlib.Path(__file__).resolve().parent.parent


def workspace_members(root):
    """Map manifest path -> package name for workspace members.

    Going through `cargo metadata` rather than guessing from the directory name means the
    --expect-* checks below name real packages, and that a crate rename shows up as an error
    instead of as a check that silently stops checking anything.
    """
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=root,
        stdout=subprocess.PIPE,
        text=True,
        check=True,
    )
    meta = json.loads(proc.stdout)
    return {pkg["manifest_path"]: pkg["name"] for pkg in meta["packages"]}


def package_label(manifest_path, members):
    name = members.get(manifest_path)
    if name is not None:
        return name
    # A registry dependency. Its directory is <name>-<version>, which is a good enough label
    # for a line in a CI log; we never assert on these, only report them.
    return pathlib.Path(manifest_path).parent.name


def cmd_run(args):
    root = repo_root()
    members = workspace_members(root)

    for relative in args.touch:
        path = root / relative
        if not path.is_file():
            sys.exit(f"rebuild-probe: nothing to touch at {relative}")
        # An mtime bump is enough to make cargo consider the file dirty, and it leaves the
        # working tree byte-identical, which matters because these paths belong to crates this
        # script has no business editing.
        path.touch()

    # json-render-diagnostics keeps rustc's own error output human-readable on stderr while
    # the artifact records come back as JSON on stdout.
    build = [
        "cargo", "build", "--workspace", "--locked",
        "--message-format=json-render-diagnostics",
    ]
    if args.timings:
        build.append("--timings")

    # stderr is left attached so that output and cargo's progress reach the CI log live.
    start = time.monotonic()
    proc = subprocess.run(build, cwd=root, stdout=subprocess.PIPE, text=True)
    elapsed_ms = round((time.monotonic() - start) * 1000)
    if proc.returncode != 0:
        sys.exit(f"rebuild-probe: cargo build failed with status {proc.returncode}")

    seen = set()
    rebuilt = set()
    for line in proc.stdout.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact":
            continue
        label = package_label(message["manifest_path"], members)
        seen.add(label)
        # A package can emit several artifacts (a lib and a bin); if any of them was not
        # served from cache, the package was recompiled.
        if not message.get("fresh", False):
            rebuilt.add(label)

    problems = []
    expected = set(args.expect_fresh) | set(args.expect_rebuilt)
    missing = sorted(expected - seen)
    if missing:
        problems.append(
            "cargo produced no artifact for " + ", ".join(missing) + " - renamed or removed?"
        )
    for name in sorted(set(args.expect_fresh) & seen):
        if name in rebuilt:
            problems.append(f"{name} was recompiled but should have stayed fresh")
    for name in sorted(set(args.expect_rebuilt) & seen):
        if name not in rebuilt:
            problems.append(f"{name} stayed fresh but should have been recompiled")
    if args.expect_all_fresh and rebuilt:
        problems.append("cargo recompiled " + ", ".join(sorted(rebuilt)) + " on a no-op build")
    if args.max_ms is not None and elapsed_ms > args.max_ms:
        problems.append(f"took {elapsed_ms} ms, ceiling is {args.max_ms} ms")

    if args.json_out:
        out = root / args.json_out
        out.parent.mkdir(parents=True, exist_ok=True)
        record = {
            "label": args.label,
            "measurement": MEASUREMENT.get(args.label, args.label),
            "elapsed_ms": elapsed_ms,
            "rebuilt": sorted(rebuilt),
            "ok": not problems,
        }
        with out.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(record) + "\n")

    summary = ", ".join(sorted(rebuilt)) if rebuilt else "nothing"
    print(f"{args.label}: {elapsed_ms} ms, recompiled {summary}", file=sys.stderr)
    for problem in problems:
        print(f"  FAIL {args.label}: {problem}", file=sys.stderr)
    return 1 if problems else 0


def describe_rebuilt(names):
    if not names:
        return "nothing"
    if len(names) <= 4:
        return ", ".join(f"`{name}`" for name in names)
    return f"{len(names)} packages"


def cmd_report(args):
    path = pathlib.Path(args.results)
    if not path.is_file():
        sys.exit(f"rebuild-probe: no results at {args.results}")

    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    print("| design.md §13 measurement | wall clock | recompiled | reference goal |")
    print("|---|---:|---|---:|")
    for row in rows:
        goal = REFERENCE_GOAL_MS.get(row["label"])
        print(
            "| {name} | {secs:.2f} s | {rebuilt} | {goal} |".format(
                name=row["measurement"],
                secs=row["elapsed_ms"] / 1000,
                rebuilt=describe_rebuilt(row["rebuilt"]),
                goal=f"< {goal / 1000:g} s" if goal else "—",
            )
        )
    print()
    print(
        "Reference goals are §13's, and §13 states them for a fixed reference machine. On a"
        " shared CI runner the wall clock is a trend at best; the *recompiled* column, which"
        " does not depend on the machine, is the column that was actually asserted on."
    )
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    run = sub.add_parser("run", help="build once and check what was recompiled")
    run.add_argument("label", choices=sorted(MEASUREMENT), help="which §13 measurement this is")
    run.add_argument("--touch", action="append", default=[], metavar="PATH",
                     help="bump this file's mtime before building (repeatable)")
    run.add_argument("--expect-fresh", action="append", default=[], metavar="PKG",
                     help="fail if this package was recompiled (repeatable)")
    run.add_argument("--expect-rebuilt", action="append", default=[], metavar="PKG",
                     help="fail if this package was not recompiled (repeatable)")
    run.add_argument("--expect-all-fresh", action="store_true",
                     help="fail if anything at all was recompiled")
    run.add_argument("--max-ms", type=int, metavar="MS",
                     help="fail if the build took longer than this")
    run.add_argument("--timings", action="store_true",
                     help="pass --timings to cargo, leaving a report in target/cargo-timings")
    run.add_argument("--json-out", metavar="PATH", help="append a JSON record to this file")
    run.set_defaults(func=cmd_run)

    report = sub.add_parser("report", help="render collected records as a markdown table")
    report.add_argument("results", help="the JSON-lines file written by `run --json-out`")
    report.set_defaults(func=cmd_report)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
