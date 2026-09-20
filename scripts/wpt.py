#!/usr/bin/env python3
"""Run the Web Platform Tests the WinterTC Minimum Common API is defined by, through both engines.

WinterTC (Ecma TC55) names the interfaces every server-side runtime shares and defines their
conformance by the Web Platform Tests; `scripts/native-surface-inventory.py` answers whether each
one is *there*, and this answers whether it *behaves*. Node runs a filtered WPT the same way, and
so do Deno, Bun and workerd.

What runs: every `*.any.js` under the pinned directories (`DIRECTORIES`), the WPT files written
to run outside a browser. Each is assembled into one CommonJS script -- `scripts/wpt-shim.js`,
`resources/testharness.js`, the `// META: script=` includes, the test -- and run with the Node
adapter, once interpreted and once natively, with a `// META: variant=` query each; the harness's
completion callback prints one JSON line per subtest.

The baseline (`conformance/wpt-baseline.json`) is per directory per engine, like the Test262
one: a chunk whose pass count falls names itself, and a whole-corpus number would hide a chunk
that lost what another gained.

    scripts/wpt.py fetch                       # the pinned checkout into ~/.cache/kiln
    scripts/wpt.py baseline [--update]         # every chunk against the baseline
    scripts/wpt.py run --only url              # one chunk, results on stdout
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

WPT_REPO = "https://github.com/web-platform-tests/wpt"
WPT_COMMIT = "986e75d7897742148c16253c08c50c8ba0e7b0f7"
# The directories the Minimum Common API's interfaces are tested in, plus what they include.
# `fetch/api` needs the WPT server's Python handlers and is left out until a server exists.
DIRECTORIES = [
    "resources",
    "common",
    "url",
    "urlpattern",
    "streams",
    "encoding",
    "dom/events",
    "dom/abort",
    "FileAPI",
    "xhr/formdata",
    "compression",
    "WebCryptoAPI",
    "html/webappapis/structured-clone",
    "html/webappapis/timers",
    "html/webappapis/microtask-queuing",
    "html/webappapis/atob",
    "console",
    "hr-time",
    "wasm/jsapi",
]
# The chunks the baseline is kept by: a test's chunk is the first of these that prefixes it.
CHUNKS = [
    "url",
    "urlpattern",
    "streams",
    "encoding",
    "dom/events",
    "dom/abort",
    "FileAPI",
    "xhr/formdata",
    "compression",
    "WebCryptoAPI",
    "html/webappapis",
    "console",
    "hr-time",
    "wasm/jsapi",
]
CACHE = Path(os.environ.get("KILN_CACHE_DIR", Path.home() / ".cache" / "kiln"))
CHECKOUT = CACHE / f"wpt-{WPT_COMMIT[:8]}"
TIMEOUT_SECONDS = 120


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, check=True, text=True, capture_output=True, **kw)


def fetch(checkout: Path) -> None:
    """A sparse, blobless clone of the pinned commit: the sixty megabytes this needs, not the
    corpus's gigabytes, and verified against the pin so the number is about the compiler."""
    if checkout.exists():
        head = run(["git", "rev-parse", "HEAD"], cwd=checkout).stdout.strip()
        if head == WPT_COMMIT:
            print(f"wpt: {checkout} already at {WPT_COMMIT[:8]}", file=sys.stderr)
            return
        shutil.rmtree(checkout)
    checkout.parent.mkdir(parents=True, exist_ok=True)
    run(["git", "clone", "--filter=blob:none", "--sparse", "--no-checkout", WPT_REPO, str(checkout)])
    run(["git", "sparse-checkout", "set", *DIRECTORIES], cwd=checkout)
    run(["git", "fetch", "--depth", "1", "origin", WPT_COMMIT], cwd=checkout)
    run(["git", "checkout", "--detach", WPT_COMMIT], cwd=checkout)
    print(f"wpt: fetched {WPT_COMMIT[:8]} into {checkout}", file=sys.stderr)


def verify(checkout: Path) -> None:
    if not checkout.exists():
        sys.exit(f"wpt: no checkout at {checkout}; run `scripts/wpt.py fetch` (mise run wpt-fetch)")
    head = run(["git", "rev-parse", "HEAD"], cwd=checkout).stdout.strip()
    if head != WPT_COMMIT:
        sys.exit(f"wpt: {checkout} is at {head[:8]}, not the pinned {WPT_COMMIT[:8]}")
    if not (checkout / "resources" / "testharness.js").exists():
        sys.exit(f"wpt: {checkout} has no resources/testharness.js; the sparse checkout is short")


META = re.compile(r"^//\s*META:\s*(\w+)=(.*)$")


def metas(source: str) -> dict[str, list[str]]:
    found: dict[str, list[str]] = {}
    for line in source.splitlines():
        m = META.match(line.strip())
        if not m:
            if line.strip() and not line.strip().startswith("//"):
                break
            continue
        found.setdefault(m.group(1), []).append(m.group(2).strip())
    return found


def discover(checkout: Path, only: str | None) -> list[Path]:
    tests = []
    for chunk in CHUNKS:
        root = checkout / chunk
        if not root.exists():
            continue
        for path in sorted(root.rglob("*.any.js")):
            rel = path.relative_to(checkout).as_posix()
            if only and only not in rel:
                continue
            tests.append(path)
    return tests


def chunk_of(rel: str) -> str:
    for chunk in CHUNKS:
        if rel.startswith(chunk + "/"):
            return chunk
    return rel.split("/")[0]


def assemble(checkout: Path, test: Path, query: str, shim: str) -> str:
    """One CommonJS file: the shim, the harness, the includes, the test. One file rather than a
    module graph because `.any.js` tests and their includes share a scope by design."""
    source = test.read_text(encoding="utf-8")
    rel = "/" + test.relative_to(checkout).as_posix()
    html_path = rel[: -len(".any.js")] + ".any.html"
    parts = [
        "const __kilnWptFs = require(\"node:fs\"); const __kilnWptPath = require(\"node:path\");",
        "globalThis.__KILN_WPT = " + json.dumps({"path": html_path, "query": query, "root": str(checkout)})
        + "; globalThis.__KILN_WPT.fs = __kilnWptFs; globalThis.__KILN_WPT.pathModule = __kilnWptPath;",
        shim,
        (checkout / "resources" / "testharness.js").read_text(encoding="utf-8"),
        # The reporter: one line per subtest, then one for the harness, all prefixed so the
        # runner can pick them out of whatever the test itself printed.
        """add_completion_callback(function (tests, status) {
  for (const t of tests) console.log("#WPT " + JSON.stringify({ name: t.name, status: t.status, message: t.message }));
  console.log("#WPT " + JSON.stringify({ harness: status.status, message: status.message }));
});""",
    ]
    for script in metas(source).get("script", []):
        include = checkout / script.lstrip("/") if script.startswith("/") else test.parent / script
        if include.exists():
            parts.append(f"// ---- {script}\n" + include.read_text(encoding="utf-8"))
        else:
            parts.append(f"throw new Error('META script not in the checkout: {script}');")
    parts.append(f"// ---- {rel}\n" + source)
    return "\n".join(parts)


class Result:
    def __init__(self, rel: str, variant: str, engine: str):
        self.rel = rel
        self.variant = variant
        self.engine = engine
        self.subtests: list[dict] = []
        self.harness: str | None = None
        self.error: str | None = None

    @property
    def passed(self) -> int:
        return sum(1 for t in self.subtests if t["status"] == 0)

    @property
    def total(self) -> int:
        return len(self.subtests)


def run_one(kiln: Path, workdir: Path, checkout: Path, shim: str, test: Path, query: str, engine: str) -> Result:
    rel = test.relative_to(checkout).as_posix()
    result = Result(rel, query, engine)
    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", rel + query)
    path = workdir / f"{safe}.cjs"
    path.write_text(assemble(checkout, test, query, shim), encoding="utf-8")
    argv = [str(kiln), "--one-shot", "--node", "run"]
    if engine == "native":
        argv.append("--native")
    argv.append(str(path))
    try:
        proc = subprocess.run(argv, capture_output=True, text=True, timeout=TIMEOUT_SECONDS, cwd=workdir)
    except subprocess.TimeoutExpired:
        result.error = "timeout"
        return result
    for line in proc.stdout.splitlines():
        if not line.startswith("#WPT "):
            continue
        try:
            record = json.loads(line[5:])
        except json.JSONDecodeError:
            continue
        if "harness" in record:
            result.harness = {0: "OK", 1: "ERROR", 2: "TIMEOUT", 3: "PRECONDITION_FAILED"}.get(record["harness"], str(record["harness"]))
        else:
            result.subtests.append(record)
    if result.harness is None:
        tail = (proc.stderr.strip().splitlines() or proc.stdout.strip().splitlines() or ["no output"])[-1]
        result.error = f"exit {proc.returncode}: {tail[:300]}"
    return result


def variants(source: str) -> list[str]:
    found = metas(source).get("variant", [])
    return found or [""]


def run_corpus(kiln: Path, checkout: Path, only: str | None, engines: list[str], jobs: int, workdir: Path) -> list[Result]:
    shim = (Path(__file__).parent / "wpt-shim.js").read_text(encoding="utf-8")
    jobs_list = []
    for test in discover(checkout, only):
        for query in variants(test.read_text(encoding="utf-8")):
            for engine in engines:
                jobs_list.append((test, query, engine))
    if not jobs_list:
        sys.exit("wpt: no tests matched; the checkout is short or `--only` matched nothing")
    print(f"wpt: {len(jobs_list)} runs ({len(jobs_list) // max(1, len(engines))} tests x {len(engines)} engine(s))", file=sys.stderr)
    results: list[Result] = []
    done = 0
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        for result in pool.map(lambda j: run_one(kiln, workdir, checkout, shim, *j), jobs_list):
            results.append(result)
            done += 1
            if done % 25 == 0:
                print(f"  {done}/{len(jobs_list)}", file=sys.stderr)
    return results


def summarize(results: list[Result]) -> dict[str, dict[str, dict[str, int]]]:
    """chunk -> engine -> {pass, fail, subtests, files, harness_errors}."""
    out: dict[str, dict[str, dict[str, int]]] = {}
    for r in results:
        chunk = chunk_of(r.rel)
        row = out.setdefault(chunk, {}).setdefault(r.engine, {"pass": 0, "fail": 0, "subtests": 0, "files": 0, "harness_errors": 0})
        row["files"] += 1
        row["pass"] += r.passed
        row["fail"] += r.total - r.passed
        row["subtests"] += r.total
        if r.error is not None or (r.harness not in (None, "OK")):
            row["harness_errors"] += 1
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("fetch")
    for name in ("run", "baseline"):
        p = sub.add_parser(name)
        p.add_argument("--kiln", default="target/release/kiln")
        p.add_argument("--only", default=None, help="a path fragment; run only the tests it matches")
        p.add_argument("--engine", choices=["interpreter", "native", "both"], default="both")
        p.add_argument("--jobs", type=int, default=4)
        p.add_argument("--checkout", default=None)
        p.add_argument("--report", default=None, help="write every subtest result here as JSON")
        if name == "baseline":
            p.add_argument("--baseline", default="conformance/wpt-baseline.json")
            p.add_argument("--update", action="store_true")
    args = parser.parse_args()
    checkout = Path(args.checkout) if getattr(args, "checkout", None) else CHECKOUT
    if args.command == "fetch":
        fetch(checkout)
        return 0
    verify(checkout)
    kiln = Path(args.kiln).resolve()
    if not kiln.exists():
        sys.exit(f"wpt: no kiln binary at {kiln}")
    engines = ["interpreter", "native"] if args.engine == "both" else [args.engine]
    with tempfile.TemporaryDirectory(prefix="kiln-wpt-") as tmp:
        results = run_corpus(kiln, checkout, args.only, engines, args.jobs, Path(tmp))
    if args.report:
        Path(args.report).write_text(json.dumps([
            {"test": r.rel, "variant": r.variant, "engine": r.engine, "harness": r.harness, "error": r.error, "subtests": r.subtests}
            for r in results
        ], indent=1))
    summary = summarize(results)
    for chunk in sorted(summary):
        for engine in engines:
            row = summary[chunk].get(engine)
            if row:
                print(f"{chunk:32} {engine:12} {row['pass']:5}/{row['subtests']:<5} pass, {row['files']} files, {row['harness_errors']} harness errors")
    if args.command == "run":
        return 0
    baseline_path = Path(args.baseline)
    if args.update:
        baseline_path.write_text(json.dumps({"commit": WPT_COMMIT, "chunks": summary}, indent=1, sort_keys=True) + "\n")
        print(f"wpt: wrote {baseline_path}", file=sys.stderr)
        return 0
    if not baseline_path.exists():
        sys.exit(f"wpt: no baseline at {baseline_path}; run with --update to create it")
    baseline = json.loads(baseline_path.read_text())
    if baseline.get("commit") != WPT_COMMIT:
        sys.exit(f"wpt: the baseline was taken at {str(baseline.get('commit'))[:8]}, this run is {WPT_COMMIT[:8]}; --update after a pin change")
    regressions = []
    gains = []
    for chunk, engines_rows in baseline["chunks"].items():
        for engine, was in engines_rows.items():
            now = summary.get(chunk, {}).get(engine)
            if now is None:
                regressions.append(f"{chunk} [{engine}]: {was['pass']} passing in the baseline, no result now")
            elif now["pass"] < was["pass"]:
                regressions.append(f"{chunk} [{engine}]: {was['pass']} -> {now['pass']} passing")
            elif now["pass"] > was["pass"]:
                gains.append(f"{chunk} [{engine}]: {was['pass']} -> {now['pass']} passing")
    for line in gains:
        print(f"gain     {line}")
    for line in regressions:
        print(f"REGRESS  {line}")
    if regressions:
        print(f"wpt: {len(regressions)} chunk(s) regressed; accept with --update if intended", file=sys.stderr)
        return 1
    if gains:
        print("wpt: gains above the baseline; record them with --update", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
