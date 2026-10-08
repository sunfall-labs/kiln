#!/usr/bin/env python3
"""Run TypeScript's own compiler and conformance tests through `kiln check`'s checker.

Bun keeps typescript-go's test suite and its committed baselines in one file,
`test/cli/check/typescript-go/bundle.zst`: TypeScript's `tests/cases/{compiler,conformance}`,
typescript-go's own tests, the library files, and what typescript-go expects of each test. Bun's
`bun check` matches all of it byte for byte. This runs every test through kiln's oxc frontend,
checker adapter and driver (the `ts_conformance` example of kiln-type-checker, a port of Bun's
runner) and grades each test's `.errors.txt` against typescript-go's:

    All     byte for byte
    Spans   all but the related information
    Words   the same errors at the same places, in the same words
    Codes   the same error codes at the same places
    Differs anything else
    Broken  the checker panicked

`conformance/ts-conformance-baseline.json` records every test below `All`, with its level. A test
that falls below its recorded level (a test not listed is recorded at `All`) is a regression and
fails the run; one that rises is reported, and `--update` records it. Moving the baseline is a diff
a reviewer accepts, next to its cause.

The bundle lives in ~/.cache/kiln/ts-conformance-<commit>, fetched once.
"""

import argparse
import collections
import json
import os
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

BUN_COMMIT = "43a42a836b7a0c53fecb25aab189fba42928780a"
BUNDLE_URL = (
    "https://raw.githubusercontent.com/oven-sh/bun/{commit}/test/cli/check/typescript-go/bundle.zst"
)
REPO = Path(__file__).resolve().parent.parent
BASELINE = REPO / "conformance" / "ts-conformance-baseline.json"
LEVELS = ["Broken", "Differs", "Codes", "Words", "Spans", "All"]
REFERENCE = "/testdata/baselines/reference"
SUITES = [
    ("compiler", "/_submodules/TypeScript/tests/cases/compiler", f"{REFERENCE}/submodule/compiler"),
    (
        "conformance",
        "/_submodules/TypeScript/tests/cases/conformance",
        f"{REFERENCE}/submodule/conformance",
    ),
    ("own-compiler", "/testdata/tests/cases/compiler", f"{REFERENCE}/compiler"),
    ("own-conformance", "/testdata/tests/cases/conformance", f"{REFERENCE}/conformance"),
]


def cache_dir() -> Path:
    return Path.home() / ".cache" / "kiln" / f"ts-conformance-{BUN_COMMIT[:10]}"


def fetch_bundle(cache: Path) -> Path:
    bundle = cache / "bundle.txt"
    if bundle.exists():
        return bundle
    cache.mkdir(parents=True, exist_ok=True)
    compressed = cache / "bundle.zst"
    if not compressed.exists():
        with urllib.request.urlopen(BUNDLE_URL.format(commit=BUN_COMMIT)) as response:
            compressed.write_bytes(response.read())
    if shutil.which("zstd") is None:
        sys.exit("ts-conformance: needs zstd to decompress the bundle")
    partial = cache / "bundle.txt.partial"
    subprocess.run(["zstd", "-q", "-d", "-f", str(compressed), "-o", str(partial)], check=True)
    partial.rename(bundle)
    return bundle


def build_runner() -> Path:
    subprocess.run(
        [
            "mbx", "build", "--release", "-p", "kiln-type-checker", "--features", "baselines",
            "--example", "ts_conformance", "--offline",
        ],
        cwd=REPO,
        check=True,
    )
    return REPO / "target" / "release" / "examples" / "ts_conformance"


def run(runner: Path, bundle: Path, report: Path, only: str | None, threads: int) -> None:
    command = [
        str(runner),
        f"--bundle={bundle}",
        "--lib=/internal/bundled/libs",
        "--testlib=/_submodules/TypeScript/tests/lib",
        f"--threads={threads}",
        f"--report={report}",
    ]
    if only:
        command.append(f"--only={only}")
    command += [f"{name}={cases}={baselines}={baselines}/names.txt" for name, cases, baselines in SUITES]
    # The runner prints its summary; its exit status says only whether everything matched.
    subprocess.run(command, check=False)
    if not report.exists():
        sys.exit("ts-conformance: the runner wrote no report")


def read_report(report: Path) -> dict:
    """name -> (level, unsupported): the first thing kiln's frontend or adapter could not
    represent, which is the cause of a difference rather than the checker."""
    results = {}
    for line in report.read_text().splitlines():
        fields = line.split("\t")
        level, name = fields[0], fields[1]
        unsupported = fields[4] if len(fields) > 4 else ""
        results[name] = (level, unsupported)
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--update", action="store_true", help="record the current levels")
    parser.add_argument("--runner", type=Path, help="a built ts_conformance example")
    parser.add_argument("--only", help="run only the tests whose path contains this text")
    parser.add_argument("--report", type=Path, help="keep the per-test report here")
    parser.add_argument("--threads", type=int, default=min(os.cpu_count() or 4, 16))
    args = parser.parse_args()
    if args.update and args.only:
        sys.exit("ts-conformance: --update records the whole suite; drop --only")

    cache = cache_dir()
    bundle = fetch_bundle(cache)
    runner = args.runner or build_runner()
    report = args.report or cache / f"report-{os.getpid()}.tsv"
    run(runner, bundle, report, args.only, args.threads)
    results = read_report(report)
    if not args.report:
        report.unlink()

    levels = collections.Counter(level for level, _ in results.values())
    total = len(results)
    unsupported = sum(1 for level, cause in results.values() if level != "All" and cause)
    print(f"ts-conformance: {total} tests (each configuration counts)")
    for level in reversed(LEVELS):
        at_least = sum(levels[l] for l in LEVELS[LEVELS.index(level):])
        print(f"  {level:8} {levels[level]:6}   at least: {at_least:6} {100 * at_least / max(total, 1):6.2f}%")
    print(f"  below All with a frontend or adapter gap: {unsupported}")

    baseline = json.loads(BASELINE.read_text()) if BASELINE.exists() else {"below_all": {}}
    recorded = baseline["below_all"]
    rank = {level: at for at, level in enumerate(LEVELS)}
    regressed, gained = [], []
    for name, (level, _) in sorted(results.items()):
        before = recorded.get(name, "All")
        if rank[level] < rank[before]:
            regressed.append(f"{name} {before}->{level}")
        elif rank[level] > rank[before]:
            gained.append(f"{name} {before}->{level}")
    print(f"ts-conformance: {len(gained)} gained, {len(regressed)} regressed against the baseline")
    if args.update:
        BASELINE.write_text(
            json.dumps(
                {
                    "bun_commit": BUN_COMMIT,
                    "tests": total,
                    "levels": {level: levels[level] for level in LEVELS},
                    "below_all": {
                        name: level for name, (level, _) in sorted(results.items()) if level != "All"
                    },
                },
                indent=1,
            )
            + "\n"
        )
        print(f"ts-conformance: wrote {BASELINE.relative_to(REPO)}")
        return 0
    if regressed:
        print("ts-conformance: regressed:\n  " + "\n  ".join(regressed[:200]))
        return 1
    if gained and not args.only:
        print("ts-conformance: improved (record with --update):\n  " + "\n  ".join(gained[:200]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
