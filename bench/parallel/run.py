#!/usr/bin/env python3
"""The parallel built-ins benchmarks: kiln's parallel family against node, bun, a node
worker_threads pool and Go, one table. See README.md.

run.sh is the entry point; this does the work. It builds every kiln program once with
`--explain-parallel` (keeping what the compiler decided per site) and the Go program once, both
outside every measurement, then runs every (workload, runtime) job once per round, rotating the
runtime order each round so a background build that starts mid-run hurts every column rather than
one. Every program times itself through bench/harness.mjs (or its Go twin): a cold call apart, then
the median of warm batches. A cell is the median of those across rounds; `~` marks one whose rounds
spread by more than NOISY of their median. A cell whose checksum differs from the reference
(node's sequential method, else Go) did different work: it is excluded and named.
"""

import argparse
import concurrent.futures
import datetime
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
JS = os.path.join(HERE, "js")
NOISY = 0.25

# (name, description, tier note for kiln's parallel column or None when every site qualifies)
WORKLOADS = [
    ("map-light", "map `x * 1.5 + 2`, Float64Array", None),
    ("map-heavy", "map `sqrt(x*x+1)*sin(x)+cos(x/2)`, Float64Array", None),
    ("filter", "filter `sin(x) * x > cut`, Float64Array", None),
    ("reduce-sum", "reduce `a + x`, Float64Array", None),
    ("reduce-fold", "reduce, 8-round integer mix per element", None),
    ("sort-typed", "sort Float64Array, default order (copy + sort)", None),
    ("sort-array", "sort number array, `(a, b) => a - b` (copy + sort)", None),
    ("pipeline-column", "pipeline: amounts -> cents (map) -> > 500.00 (filter) -> total (reduce)", None),
    ("sort-records", "sort `{id, price}` records, `(a, b) => a.price - b.price` (copy + sort)", None),
    ("pipeline-records", "pipeline: score records held as typed-array columns -> filter -> total", None),
    ("pipeline-objects", "pipeline: score `{price, qty, rating}` objects -> filter -> total", None),
    ("enrich-records", "tier 3: `records.map(r => ({ ...r, total: r.price * r.qty }))`", None),
    ("flatmap-records", "tier 3: `records.flatMap(r => r.qty > 4 ? [r.price, r.qty] : [])`", None),
    ("format-csv", "tier 3: `records.map(r => `${r.id},${r.qty},${r.price}`)`", None),
    ("parse-csv", "tier 4: `lines.map(l => l.trim().split(\",\").map(Number))`", None),
]

# Tier-3 and tier-4 workloads have no `worker_threads` pool or Go twin yet: their pool and Go cells are `--`.
KILN_NODE_ONLY = {"enrich-records", "flatmap-records", "format-csv", "parse-csv"}

# Workloads a realistic program needs that no kiln parallel lowering can take yet. Not measured:
# the row is kept so the table grows into it as each tier lands (PRD tiers 2-4).
WAITING = []


def config(quick):
    if quick:
        return dict(rounds=1, budget_ms=400, n=1000000)
    return dict(rounds=3, budget_ms=1000, n=1000000)


def log(*parts):
    print(*parts, file=sys.stderr, flush=True)


def sh(argv):
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=60).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "unavailable"


# ------------------------------------------------------------------------------------------------
# building

SITE = re.compile(r"^(\S+:\d+:\d+)\s+(parallel\w+)\s+(.*)$")


def build_kiln(kiln, name, work):
    source = os.path.join("bench", "parallel", "js", name + ".mjs")
    out = os.path.join(work, f"kiln-{name}")
    argv = [kiln, "--one-shot", "--node", "build", source, "--profile", "native-release",
            "--output", out, "--explain-parallel"]
    done = subprocess.run(argv, capture_output=True, text=True, cwd=REPO)
    text = done.stdout + done.stderr
    if done.returncode != 0 or not os.path.exists(out):
        first = next((l for l in text.splitlines() if "error" in l.lower()), text.strip()[-300:])
        return name, None, first, []
    sites, lines = [], text.splitlines()
    for i, line in enumerate(lines):
        match = SITE.match(line)
        if match:
            reason = ""
            if i + 1 < len(lines) and lines[i + 1].startswith("  ") and not SITE.match(lines[i + 1]):
                reason = lines[i + 1].strip()
            sites.append({"at": match.group(1), "method": match.group(2), "verdict": match.group(3), "reason": reason})
    return name, out, None, sites


def build_all(kiln, go_cmd, work):
    bins, failures, sites = {}, {}, {}
    log("  kiln build (native-release, --explain-parallel), 3 at a time")
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        for name, out, error, found in pool.map(lambda w: build_kiln(kiln, w[0], work), WORKLOADS):
            bins[name] = out
            sites[name] = found
            if error:
                failures[name] = error
                log(f"    {name}: build failed: {error}")
    log("  go build")
    gobin = os.path.join(work, "go-parallel")
    done = subprocess.run(go_cmd + ["build", "-o", gobin, "."], capture_output=True, text=True,
                          cwd=os.path.join(HERE, "go"))
    if done.returncode != 0:
        failures["go"] = (done.stderr.strip().splitlines() or ["go build failed"])[-1]
        gobin = None
    return bins, gobin, failures, sites


# ------------------------------------------------------------------------------------------------
# running


def run_one(argv, env, timeout):
    try:
        done = subprocess.run(argv, capture_output=True, text=True, env=env, cwd=REPO, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"error": f"timeout after {timeout}s"}
    for line in done.stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if "warm_median" in record:
                return record
    tail = (done.stderr.strip() or done.stdout.strip() or f"exit {done.returncode}").splitlines()
    return {"error": f"exit {done.returncode}: " + (tail[-1] if tail else "")}


def columns(cores):
    return [
        ("node", "node `map`/…"),
        ("bun", "bun `map`/…"),
        ("pool", f"node `worker_threads` pool ({cores})"),
        ("go", f"Go goroutines ({cores})"),
        ("kiln", "kiln `map`/…"),
        ("par1", "kiln `parallel*` @1"),
        ("par4", "kiln `parallel*` @4"),
        ("parN", f"kiln `parallel*` @{cores}"),
    ]


def jobs_for(cfg, bins, gobin, cores):
    n, budget = str(cfg["n"]), str(cfg["budget_ms"])
    base = dict(os.environ)
    jobs = []
    for name, _, _ in WORKLOADS:
        js = os.path.join(JS, name + ".mjs")
        jobs.append((name, "node", ["node", js, "seq", n, budget], base))
        jobs.append((name, "bun", ["bun", js, "seq", n, budget], base))
        if name not in KILN_NODE_ONLY:
            jobs.append((name, "pool", ["node", os.path.join(JS, "pool.mjs"), name, n, budget, str(cores)], base))
        if gobin and name not in KILN_NODE_ONLY:
            jobs.append((name, "go", [gobin, name, n, budget], dict(base, GOMAXPROCS=str(cores))))
        binary = bins.get(name)
        if binary:
            jobs.append((name, "kiln", [binary, "seq", n, budget], base))
            for col, threads in (("par1", 1), ("par4", 4), ("parN", cores)):
                jobs.append((name, col, [binary, "par", n, budget], dict(base, KILN_PARALLELISM=str(threads))))
    return jobs


# ------------------------------------------------------------------------------------------------
# aggregation


def normalise(checksum):
    """Each `:`-separated number at twelve significant digits (see js/common.mjs)."""
    parts = []
    for part in str(checksum).split(":"):
        try:
            parts.append(f"{float(part):.12g}")
        except ValueError:
            parts.append(part)
    return ":".join(parts)


def aggregate(results):
    table = {}
    for r in results:
        cell = table.setdefault((r["workload"], r["column"]), {"values": [], "colds": [], "checksums": set(), "errors": []})
        if "error" in r:
            cell["errors"].append(r["error"])
            continue
        cell["values"].append(r["warm_median"])
        cell["colds"].append(r.get("cold", 0))
        cell["checksums"].add(normalise(r["checksum"]))
    for cell in table.values():
        v = cell["values"]
        if v:
            mid = statistics.median(v)
            cell["median"] = mid
            cell["spread"] = (max(v) - min(v)) / mid if mid else 0.0
            cell["cold"] = statistics.median(cell["colds"])
    return table


def divergences(table):
    out = {}
    for name, _, _ in WORKLOADS:
        ref = None
        for col in ("node", "go"):
            cell = table.get((name, col))
            if cell and len(cell["checksums"]) == 1:
                ref = (col, next(iter(cell["checksums"])))
                break
        for (w, col), cell in table.items():
            if w != name:
                continue
            if len(cell["checksums"]) > 1:
                out[(w, col)] = f"checksum changed between rounds: {sorted(cell['checksums'])}"
            elif ref and cell["checksums"] and next(iter(cell["checksums"])) != ref[1]:
                out[(w, col)] = f"checksum {next(iter(cell['checksums']))} != {ref[0]}'s {ref[1]}"
    return out


# ------------------------------------------------------------------------------------------------
# report


def fmt_ms(value):
    if value >= 100:
        return f"{value:,.0f}"
    if value >= 10:
        return f"{value:.1f}"
    if value >= 1:
        return f"{value:.2f}"
    return f"{value:.3f}"


def machine_info(kiln, go_cmd):
    cpu = "unknown"
    try:
        with open("/proc/cpuinfo") as handle:
            for line in handle:
                if line.startswith("model name"):
                    cpu = line.split(":", 1)[1].strip()
                    break
    except OSError:
        cpu = platform.processor() or "unknown"
    mem = "unknown"
    try:
        with open("/proc/meminfo") as handle:
            kib = int(handle.readline().split()[1])
            mem = f"{kib / 1024 / 1024:.1f} GiB"
    except (OSError, ValueError, IndexError):
        pass
    commit = sh(["git", "-C", REPO, "rev-parse", "--short", "HEAD"])
    dirty = sh(["git", "-C", REPO, "status", "--porcelain", "--untracked-files=no"])
    return {
        "cpu": f"{cpu}, {os.cpu_count()} hardware threads",
        "memory": mem,
        "kernel": " ".join(platform.uname()[i] for i in (0, 2, 4)),
        "kiln": f"{commit}{' (dirty)' if dirty else ''}, `{os.path.relpath(kiln, REPO)}`, programs built "
                "`--node --profile native-release`; " + sh(["clang", "--version"]).splitlines()[0],
        "node": sh(["node", "--version"]),
        "bun": sh(["bun", "--version"]),
        "go": sh(go_cmd + ["version"]).replace("go version ", ""),
    }


def render(table, diverged, failures, sites, machine, cfg, loads, started, finished, quick, cores):
    cols = columns(cores)
    noisy = 0
    L = []
    w = L.append
    w(f"# Parallel built-ins benchmarks, {started[:10]}")
    w("")
    w(f"Produced by `bench/parallel/run.sh{' --quick' if quick else ''}`; `bench/parallel/README.md` says what each "
      f"row and column measures. {cfg['n']:,} elements. Each cell is milliseconds per call, warm (the median of "
      f"calibrated batches inside one process, {cfg['budget_ms']} ms budget per run), then the median of "
      f"{cfg['rounds']} interleaved round(s); `~` marks a cell whose rounds spread by more than {int(NOISY * 100)}% "
      "of the median. Lower is better. The last two columns are kiln's all-core parallel method against kiln's own "
      "sequential method and against node's.")
    w("")
    w("## Machine")
    w("")
    for k, v in machine.items():
        w(f"- **{k}**: {v}")
    w(f"- **started / finished**: {started} / {finished}")
    w(f"- **load average (1 min) seen before each job**: min {min(loads):.1f}, median {statistics.median(loads):.1f}, "
      f"max {max(loads):.1f} on {os.cpu_count()} hardware threads. The machine is shared: other agents' builds run "
      "alongside, so single cells move by tens of percent between runs; ratios within a row are the reliable part.")
    w("")
    w("## Results (ms per call)")
    w("")
    w("| workload | " + " | ".join(label for _, label in cols) + f" | kiln @{cores} vs kiln seq | kiln @{cores} vs node |")
    w("|---" * (len(cols) + 3) + "|")
    for name, desc, tier in WORKLOADS:
        cells = []
        for col, _ in cols:
            cell = table.get((name, col))
            if cell is None:
                cells.append("--")
            elif (name, col) in diverged:
                cells.append("excluded (divergence)")
            elif "median" not in cell:
                cells.append("failed: " + (cell["errors"][0] if cell["errors"] else "?")[:60].replace("|", "/"))
            else:
                text = fmt_ms(cell["median"])
                if len(cell["values"]) > 1 and cell["spread"] > NOISY:
                    noisy += 1
                    text = "~" + text
                cells.append(text)
        if tier:
            for i, (col, _) in enumerate(cols):
                if col.startswith("par") and not cells[i].startswith(("failed", "excluded", "--")):
                    cells[i] += " (partial)"

        def ratio(a, b):
            ca, cb = table.get((name, a)), table.get((name, b))
            if not ca or not cb or "median" not in ca or "median" not in cb or (name, a) in diverged or (name, b) in diverged:
                return "--"
            return f"{ca['median'] / cb['median']:.1f}x"
        w(f"| `{name}`: {desc} | " + " | ".join(cells) + f" | {ratio('kiln', 'parN')} | {ratio('node', 'parN')} |")
    w("")
    partial = [(n, t) for n, _, t in WORKLOADS if t]
    if partial:
        w("\"(partial)\": not every stage qualifies today. " + "; ".join(f"`{n}`: {t}" for n, t in partial) + ".")
        w("")
    if noisy:
        w(f"{noisy} cell(s) marked `~` spread by more than {int(NOISY * 100)}% across rounds.")
        w("")
    if WAITING:
        w("### Waiting on a later tier (not measured yet)")
        w("")
        w("Realistic workloads no kiln parallel lowering can take today. Each row becomes a measured row, with "
          "every column above, when its tier lands; until then kiln runs the sequential method for them.")
        w("")
        w("| workload | kiln `parallel*` | why |")
        w("|---|---|---|")
        for desc, tier, why in WAITING:
            w(f"| {desc} | {tier} | {why} |")
        w("")
    w("## What ran in parallel (`kiln build --explain-parallel`)")
    w("")
    w("```")
    for name, _, _ in WORKLOADS:
        for site in sites.get(name, []):
            w(f"{site['at']}  {site['method']}  {site['verdict']}")
            if site["reason"]:
                w(f"  {site['reason']}")
        if not sites.get(name) and name in failures:
            w(f"{name}: build failed: {failures[name]}")
    w("```")
    w("")
    w("## Divergences")
    w("")
    if diverged:
        for (name, col), why in sorted(diverged.items()):
            w(f"- `{name}` / {dict(cols)[col]}: {why}")
    else:
        w("None: every cell's checksum matched node's sequential method at twelve significant digits.")
    errors = [(k, c["errors"][0]) for k, c in sorted(table.items()) if c["errors"]]
    if failures or errors:
        w("")
        w("## Failures")
        w("")
        for name, why in failures.items():
            w(f"- build `{name}`: {why}")
        for (name, col), why in errors:
            w(f"- `{name}` / {dict(cols)[col]}: {why}")
    w("")
    return "\n".join(L)


# ------------------------------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--out", help="results markdown (default: bench/parallel/results-<date>.md)")
    parser.add_argument("--only", help="comma-separated workload names")
    parser.add_argument("--kiln", default=os.path.join(REPO, "target", "release", "kiln"))
    parser.add_argument("--go", default=os.environ.get("GO_CMD", ""))
    args = parser.parse_args()
    cfg = config(args.quick)
    cores = os.cpu_count() or 1
    go_cmd = args.go.split() if args.go else None
    if go_cmd is None:
        works = shutil.which("go") and subprocess.run(["go", "version"], capture_output=True).returncode == 0
        go_cmd = ["go"] if works else ["mise", "exec", "go@latest", "--", "go"]
    global WORKLOADS
    if args.only:
        wanted = set(args.only.split(","))
        unknown = wanted - {w[0] for w in WORKLOADS}
        if unknown:
            parser.error(f"unknown workload(s): {', '.join(sorted(unknown))}")
        WORKLOADS = [w for w in WORKLOADS if w[0] in wanted]

    started = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
    work = os.path.join(REPO, "target", "bench-parallel", started.replace(":", ""))
    os.makedirs(work, exist_ok=True)
    log(f"bench/parallel: building into {os.path.relpath(work, REPO)}")
    bins, gobin, failures, sites = build_all(args.kiln, go_cmd, work)
    machine = machine_info(args.kiln, go_cmd)
    jobs = jobs_for(cfg, bins, gobin, cores)

    results, loads = [], []
    for round_index in range(cfg["rounds"]):
        for name, _, _ in WORKLOADS:
            group = [j for j in jobs if j[0] == name]
            shift = round_index % max(1, len(group))
            for _, col, argv, env in group[shift:] + group[:shift]:
                loads.append(os.getloadavg()[0])
                t = time.time()
                record = run_one(argv, env, timeout=240)
                record.update(workload=name, column=col, round=round_index, load1=loads[-1])
                results.append(record)
                shown = record.get("error") or f"{record['warm_median']:.3f} ms"
                log(f"  round {round_index + 1} {name:<17} {col:<5} {time.time() - t:5.1f}s  load {loads[-1]:4.1f}  {shown}")
    finished = datetime.datetime.now().astimezone().isoformat(timespec="seconds")

    raw = os.path.join(work, "raw.json")
    with open(raw, "w") as handle:
        json.dump({"machine": machine, "config": cfg, "results": results, "sites": sites,
                   "build_failures": failures}, handle, indent=1)
    table = aggregate(results)
    diverged = divergences(table)
    text = render(table, diverged, failures, sites, machine, cfg, loads or [0.0], started, finished, args.quick, cores)
    out = args.out or os.path.join(HERE, f"results-{started[:10]}.md")
    with open(out, "w") as handle:
        handle.write(text)
    log(f"bench/parallel: wrote {os.path.relpath(out, REPO)} (raw: {os.path.relpath(raw, REPO)})")
    print(text)


if __name__ == "__main__":
    main()
