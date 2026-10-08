#!/usr/bin/env python3
"""Compare kiln's source checker with `tsc` 7.0.2 on Bun's differential corpus.

Bun keeps `test/cli/check/differential-cases.json`: small programs about which `bun check` and
`tsc` once disagreed, each with its tsconfig. This runs every case through `tsc --pretty false`
and through kiln's checker (the `tsc_differential` example of kiln-type-checker, which prints a
project's diagnostics the way `tsc --pretty false` does), and compares the sorted lines.

`conformance/tsc-differential-baseline.json` names the cases that match. A case in it that stops
matching is a regression and fails the run; a case that starts matching is reported, and
`--update` records it. Moving the baseline is a diff a reviewer accepts, next to its cause.

The corpus and `tsc` live in ~/.cache/kiln/tsc-differential-<commit>, fetched once.
`tsc -b` cases (which build project references) are not compared: kiln has no build mode.
"""

import argparse
import concurrent.futures
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

BUN_COMMIT = "43a42a836b7a0c53fecb25aab189fba42928780a"
TSC_VERSION = "7.0.2"
CORPUS_URL = (
    "https://raw.githubusercontent.com/oven-sh/bun/{commit}/test/cli/check/differential-cases.json"
)
REPO = Path(__file__).resolve().parent.parent
BASELINE = REPO / "conformance" / "tsc-differential-baseline.json"


def cache_dir() -> Path:
    return Path.home() / ".cache" / "kiln" / f"tsc-differential-{BUN_COMMIT[:10]}"


def fetch_corpus(cache: Path) -> dict:
    corpus = cache / "cases.json"
    if not corpus.exists():
        cache.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(CORPUS_URL.format(commit=BUN_COMMIT)) as response:
            corpus.write_bytes(response.read())
    return json.loads(corpus.read_text())


def tsc_binary(cache: Path) -> Path:
    system = {"Linux": "linux", "Darwin": "darwin"}[platform.system()]
    arch = {"x86_64": "x64", "AMD64": "x64", "arm64": "arm64", "aarch64": "arm64"}[platform.machine()]
    root = cache / "tsc"
    binary = root / "node_modules" / "@typescript" / f"typescript-{system}-{arch}" / "lib" / "tsc"
    if not binary.exists():
        root.mkdir(parents=True, exist_ok=True)
        (root / "package.json").write_text(json.dumps({"dependencies": {"typescript": TSC_VERSION}}))
        installer = shutil.which("bun") or shutil.which("npm")
        if installer is None:
            sys.exit("tsc-differential: needs bun or npm to install typescript@" + TSC_VERSION)
        subprocess.run([installer, "install"], cwd=root, check=True, stdout=subprocess.DEVNULL)
    return binary


def materialize(cases: dict, into: Path) -> None:
    if (into / ".complete").exists():
        return
    shutil.rmtree(into, ignore_errors=True)
    for name, case in cases.items():
        for path, text in case["files"].items():
            target = into / name / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
    (into / ".complete").write_text("")


def tsc_outputs(cases: dict, cache: Path, names: list) -> None:
    tsc = tsc_binary(cache)
    out = cache / f"tsc-{TSC_VERSION}-out"
    out.mkdir(exist_ok=True)
    sources = cache / "cases-tsc"
    materialize(cases, sources)

    def run(name: str) -> None:
        if (out / name).exists():
            return
        result = subprocess.run(
            [str(tsc), "--pretty", "false", "--singleThreaded"],
            cwd=sources / name,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=120,
        )
        (out / name).write_text(result.stdout)

    with concurrent.futures.ThreadPoolExecutor(os.cpu_count() or 4) as pool:
        list(pool.map(run, names))


def build_harness() -> Path:
    subprocess.run(
        ["mbx", "build", "--release", "-p", "kiln-type-checker", "--example", "tsc_differential",
         "--offline"],
        cwd=REPO,
        check=True,
    )
    return REPO / "target" / "release" / "examples" / "tsc_differential"


def kiln_outputs(harness: Path, cache: Path, names: list) -> Path:
    sources = cache / "cases"
    out = cache / f"kiln-out-{os.getpid()}"
    shutil.rmtree(out, ignore_errors=True)
    env = dict(os.environ, KILN_JOBS="1", CASE_THREADS=str(os.cpu_count() or 4))
    subprocess.run(
        [str(harness), str(out)] + [str(sources / name) for name in names],
        env=env,
        check=True,
    )
    return out


def normalized(text: str, root: Path) -> list:
    prefix = re.compile(re.escape(str(root)) + r"/cases(-tsc)?/[^/]+/")
    return sorted(
        prefix.sub("", line.rstrip())
        for line in text.splitlines()
        if line.strip() and not line.startswith("kiln:")
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--update", action="store_true", help="record the matching cases")
    parser.add_argument("--harness", type=Path, help="a built tsc_differential example")
    args = parser.parse_args()

    cache = cache_dir()
    cases = fetch_corpus(cache)
    names = sorted(name for name, case in cases.items() if not case.get("build"))
    materialize(cases, cache / "cases")
    tsc_outputs(cases, cache, names)
    harness = args.harness or build_harness()
    out = kiln_outputs(harness, cache, names)

    root = cache.resolve()
    matching = []
    for name in names:
        expected = normalized((cache / f"tsc-{TSC_VERSION}-out" / name).read_text(), root)
        actual_path = out / name
        actual = normalized(actual_path.read_text(), root) if actual_path.exists() else None
        if expected == actual:
            matching.append(name)
    shutil.rmtree(out, ignore_errors=True)

    baseline = json.loads(BASELINE.read_text()) if BASELINE.exists() else {"cases": []}
    recorded = set(baseline["cases"])
    regressed = sorted(recorded - set(matching))
    gained = sorted(set(matching) - recorded)
    print(
        f"tsc-differential: {len(matching)} of {len(names)} cases match tsc {TSC_VERSION} "
        f"(baseline {len(recorded)}; {len(gained)} gained, {len(regressed)} regressed)"
    )
    if args.update:
        BASELINE.write_text(
            json.dumps(
                {
                    "bun_commit": BUN_COMMIT,
                    "tsc": TSC_VERSION,
                    "compared": len(names),
                    "matching": len(matching),
                    "cases": matching,
                },
                indent=1,
            )
            + "\n"
        )
        print(f"tsc-differential: wrote {BASELINE.relative_to(REPO)}")
        return 0
    if regressed:
        print("tsc-differential: no longer matching: " + " ".join(regressed))
        return 1
    if gained:
        print("tsc-differential: newly matching (record with --update): " + " ".join(gained))
    return 0


if __name__ == "__main__":
    sys.exit(main())
