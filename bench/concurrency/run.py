#!/usr/bin/env python3
"""The concurrency benchmarks: kiln against node, bun and Go, one table. See README.md.

run.sh is the entry point; this does the work. It builds the kiln native binaries and the Go
program once, outside every measurement, then runs every (workload, runtime) job once per round,
rotating the runtime order each round so a background build that starts mid-run hurts every
column rather than one. Each cell is the median across rounds; a cell whose rounds spread by more
than NOISY of their median is marked with "~". A case whose checksum differs from the reference
runtime's did different work: it is excluded from the table and named under "Divergences".

Programs print JSON lines (`{"case": ..., <metrics>, "checksum": ...}`). Memory the programs
cannot see themselves -- peak resident set of a whole run, the proportional set of a process
tree -- is measured here, from outside.
"""

import argparse
import datetime
import json
import os
import platform
import select
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
JS = os.path.join(HERE, "js")
PORT = 18190
NOISY = 0.25

# ------------------------------------------------------------------------------------------------
# configuration


def config(quick):
    if quick:
        return dict(
            rounds=1, start_warm=10, mem_ns=[1, 10, 100], kiln_ns=[1, 2, 10, 100], kiln_reps=5,
            msg_budget=300, oneway=100000, inrealm_oneway=10000,
            actor_graph_samples=5, actor_ns=[10, 100, 1000], shared_ns=[4], queries=2000000,
            task_ns=[10, 1000, 100000], realm_ns=[10, 1000], ticks=10, interval=10,
            pmap_n=10000000, pmap_reps=2,
            server_s=3, server_c=128,
        )
    return dict(
        rounds=3, start_warm=20, mem_ns=[1, 10, 100], kiln_ns=[1, 2, 10, 100], kiln_reps=10,
        msg_budget=800, oneway=100000, inrealm_oneway=10000,
        actor_graph_samples=10, actor_ns=[10, 100, 1000, 10000], shared_ns=[4, 16], queries=2000000,
        task_ns=[10, 1000, 100000], realm_ns=[10, 1000, 100000], ticks=10, interval=10,
        pmap_n=10000000, pmap_reps=3,
        server_s=5, server_c=128,
    )


# ------------------------------------------------------------------------------------------------
# processes and memory


def log(*parts):
    print(*parts, file=sys.stderr, flush=True)


def run_program(argv, env=None, timeout=240):
    """Run to completion. Returns (records, wall_ms, maxrss_kib, returncode, stderr_tail).

    stdout goes to a file rather than a pipe so that os.wait4 can collect the child's rusage
    (ru_maxrss: the peak resident set of the process, in KiB on Linux) without a reader thread.
    """
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        started = time.perf_counter()
        proc = subprocess.Popen(argv, stdout=out, stderr=err, env=env, cwd=REPO)
        deadline = started + timeout
        status, usage = None, None
        while True:
            pid, st, ru = os.wait4(proc.pid, os.WNOHANG)
            if pid != 0:
                status, usage = st, ru
                break
            if time.perf_counter() > deadline:
                kill_tree(proc.pid)
                _, status, usage = os.wait4(proc.pid, 0)
                status = "timeout"
                break
            time.sleep(0.002)
        wall = (time.perf_counter() - started) * 1000.0
        proc.returncode = 0  # reaped by hand; keep Popen from trying again
        out.seek(0)
        err.seek(0)
        stdout = out.read().decode("utf-8", "replace")
        stderr = err.read().decode("utf-8", "replace")
    records = []
    for line in stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                records.append(json.loads(line))
            except ValueError:
                pass
    code = status if status == "timeout" else os.waitstatus_to_exitcode(status)
    return records, wall, usage.ru_maxrss, code, stdout, stderr.strip()[-400:]


def children_map():
    kids = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat") as handle:
                fields = handle.read().rsplit(")", 1)[1].split()
            kids.setdefault(int(fields[1]), []).append(int(entry))
        except OSError:
            pass
    return kids


def tree(root):
    kids = children_map()
    out, todo = [], [root]
    while todo:
        pid = todo.pop()
        out.append(pid)
        todo.extend(kids.get(pid, []))
    return out


def tree_memory(root):
    """(pss_kib, rss_kib, processes) summed over a process and its descendants.

    PSS divides each shared page among the processes mapping it, so a sum of PSS is what the tree
    really costs; a sum of RSS counts the shared binary text once per process.
    """
    pss = rss = count = 0
    for pid in tree(root):
        try:
            with open(f"/proc/{pid}/smaps_rollup") as handle:
                for line in handle:
                    if line.startswith("Pss:"):
                        pss += int(line.split()[1])
                    elif line.startswith("Rss:"):
                        rss += int(line.split()[1])
            count += 1
        except OSError:
            pass
    return pss, rss, count


def kill_tree(root, sig=signal.SIGTERM):
    pids = tree(root)
    for pid in pids:
        try:
            os.kill(pid, sig)
        except OSError:
            pass
    return pids


def port_open(port):
    with socket.socket() as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def http_ok(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.5) as sock:
            sock.sendall(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            return b"200" in sock.recv(64)
    except OSError:
        return False


# ------------------------------------------------------------------------------------------------
# building


def build_all(kiln, go_cmd, work, selected):
    """Build the selected workloads' binaries once. Returns paths and failures."""
    binaries, failures = {}, {}
    by_workload = {
        "start": ["kiln-start", "start", "actor-start", "actor-capacity"],
        "messages": ["inrealm", "messages", "actor-graph"],
        "shared": ["shared"],
        "tasks": ["tasks"],
        "realms": ["realm-periodic"],
        "pmap": ["pmap", "pmap-parallel", "pmap-parallel-light"],
        "server": ["serve"],
    }
    for names in by_workload.values():
        binaries.update(dict.fromkeys(names))
    kiln_programs = [name for workload, names in by_workload.items()
                     if workload in selected for name in names]
    for name in kiln_programs:
        out = os.path.join(work, f"kiln-{name}")
        argv = [kiln, "--one-shot"]
        if name != "realm-periodic":
            argv.append("--node")
        argv.extend(["build", os.path.join(JS, name + ".mjs"),
                     "--profile", "native-release", "--output", out])
        log(f"  kiln build {name}.mjs")
        done = subprocess.run(argv, capture_output=True, text=True, cwd=REPO)
        if done.returncode == 0 and os.path.exists(out):
            binaries[name] = out
        else:
            text = done.stderr + done.stdout
            first = next((line for line in text.splitlines() if "error" in line.lower()), text.strip()[:300])
            failures[name] = first.strip()[:300]
            binaries[name] = None
            log(f"    refused: {failures[name]}")
    if selected - {"realms"}:
        out = os.path.join(work, "concurrency-go")
        log("  go build")
        done = subprocess.run(go_cmd + ["build", "-o", out, "."], capture_output=True, text=True,
                              cwd=os.path.join(HERE, "go"))
        if done.returncode == 0:
            binaries["go"] = out
        else:
            failures["go"] = done.stderr.strip()[:300]
            binaries["go"] = None
            log(f"    go build failed: {failures['go']}")
    return binaries, failures


# ------------------------------------------------------------------------------------------------
# jobs: each returns a list of result records tagged with runtime


def js(runtime, script, *args):
    if runtime == "node":
        return ["node", os.path.join(JS, script), *map(str, args)]
    return ["bun", os.path.join(JS, script), *map(str, args)]


def tag(records, runtime, **extra):
    for record in records:
        record["runtime"] = runtime
        record.update(extra)
    return records


def simple(runtime, argv, env=None, timeout=240, maxrss_field=None):
    records, wall, maxrss, code, stdout, err = run_program(argv, env=env, timeout=timeout)
    if code != 0 or not records:
        what = "timeout" if code == "timeout" else f"exit {code}"
        return [{"runtime": runtime, "error": f"{what}: {err or stdout[-200:]}", "argv": " ".join(argv[:3])}]
    for record in records:
        record["maxrss_kib"] = maxrss
        record["wall_ms_outside"] = wall
    return tag(records, runtime)


def available_memory_kib():
    """Use the smaller of host availability and the remaining cgroup allowance."""
    candidates = []
    try:
        with open("/proc/meminfo") as handle:
            for line in handle:
                if line.startswith("MemAvailable:"):
                    candidates.append(int(line.split()[1]))
                    break
    except OSError:
        pass
    try:
        with open("/sys/fs/cgroup/memory.max") as handle:
            limit = handle.read().strip()
        if limit != "max":
            with open("/sys/fs/cgroup/memory.current") as handle:
                used = int(handle.read())
            candidates.append(max(0, (int(limit) - used) // 1024))
    except OSError:
        pass
    return min(candidates) if candidates else None


def actor_capacity(binary, counts):
    """Keep public actors alive while checking replies, RSS and bounded scheduler threads."""
    records = []
    observed = []
    for n in counts:
        case = f"actor-capacity-{n}"
        if n >= 10000 and (len(observed) < 2 or observed[-1][0] < 1000):
            records.append({"case": case, "runtime": "kiln", "error":
                            "skipped: smaller actor counts did not establish a memory slope"})
            continue
        if n >= 10000 and len(observed) >= 2:
            (low_n, low_rss), (high_n, high_rss) = observed[-2:]
            slope = max(0, (high_rss - low_rss) / (high_n - low_n))
            need = high_rss + slope * (n - high_n)
            available = available_memory_kib()
            if available is not None and need * 1.5 > available * 0.7:
                records.append({"case": case, "runtime": "kiln", "error":
                                f"skipped: estimated {need / 1024 / 1024:.1f} GiB needs more than "
                                f"70% of {available / 1024 / 1024:.1f} GiB available"})
                continue
        result = simple("kiln", [binary, str(n)], timeout=600)
        if not result or result[0].get("case") != case:
            error = (result[0].get("error", "missing actor capacity result")
                     if result else "missing actor capacity result")
            records.append({"case": case, "runtime": "kiln", "error": error})
            continue
        record = result[0]
        records.append(record)
        rss = record.get("held_rss_kib", -1)
        if rss >= 0:
            observed.append((n, rss))
    return records


def realm_scaling(binary, counts):
    """End-to-end native realm turns, including the launcher's per-realm result files."""
    records = []
    observed = []
    for n in counts:
        case = f"realms-{n}"
        if n >= 100000 and len(observed) < 2:
            records.append({"case": case, "runtime": "kiln", "error":
                            "skipped: smaller realm counts did not establish a memory slope"})
            continue
        if n >= 100000:
            (low_n, low_rss), (high_n, high_rss) = observed[-2:]
            slope = max(0, (high_rss - low_rss) / (high_n - low_n))
            need = high_rss + slope * (n - high_n)
            available = available_memory_kib()
            if available is not None and need * 1.5 > available * 0.7:
                records.append({"case": case, "runtime": "kiln", "error":
                                f"skipped: estimated {need / 1024 / 1024:.1f} GiB needs more than "
                                f"70% of {available / 1024 / 1024:.1f} GiB available"})
                continue
        with tempfile.TemporaryDirectory(prefix="kiln-realm-scale-") as out:
            env = dict(os.environ, KILN_REALM_SCHEDULER="1", KILN_REALMS=str(n),
                       KILN_REALMS_OUT=out)
            _, wall, rss, code, stdout, err = run_program([binary], env=env, timeout=600)
            failure = None
            if code != 0:
                failure = f"exit {code}: {err or stdout[-200:]}"
            else:
                for index in range(n):
                    try:
                        with open(os.path.join(out, f"{index}.status")) as handle:
                            status = handle.read().strip()
                        with open(os.path.join(out, f"{index}.out")) as handle:
                            answer = handle.read()
                    except OSError as error:
                        failure = f"realm {index}: {error}"
                        break
                    if status != "0" or answer != "10\n":
                        failure = f"realm {index}: status {status}, output {answer[:80]!r}"
                        break
            if failure:
                records.append({"case": case, "runtime": "kiln", "error": failure})
            else:
                observed.append((n, rss))
                records.append({"case": case, "runtime": "kiln", "wall_ms": wall,
                                "maxrss_kib": rss, "turns_per_s": n * 10 / (wall / 1000),
                                "checksum": str(n * 10)})
    return records


def kiln_process_start(binary, n, reps):
    """KILN_WORKERS=n: spawn to exit, one 'ready <index>' line per worker (the message back)."""
    env = dict(os.environ, KILN_WORKERS=str(n))
    walls, checksum = [], None
    for _ in range(reps + 1):
        records, wall, _, code, stdout, err = run_program([binary], env=env, timeout=60)
        if code != 0:
            return [{"runtime": "kiln", "case": f"proc-start-{n}", "error": f"exit {code}: {err}"}]
        indexes = [int(line.split()[1]) for line in stdout.splitlines() if line.startswith("ready ")]
        checksum = str(sum(i + 1 for i in indexes)) if len(indexes) == n else f"lines={len(indexes)}"
        walls.append(wall)
    return [{"runtime": "kiln", "case": f"proc-start-{n}", "cold_ms": walls[0],
             "median_ms": statistics.median(walls[1:]), "checksum": checksum}]


def process_start(runtime, argv, reps):
    """Context row: a whole process of the runtime, spawn to exit, printing one line."""
    walls = []
    for _ in range(reps + 1):
        _, wall, _, code, _, err = run_program(argv, timeout=60)
        if code != 0:
            return [{"runtime": runtime, "case": "proc-start-1", "error": f"exit {code}: {err}"}]
        walls.append(wall)
    return [{"runtime": runtime, "case": "proc-start-1", "cold_ms": walls[0],
             "median_ms": statistics.median(walls[1:]), "checksum": "1"}]


def kiln_process_memory(binary, n):
    """KILN_WORKERS=n held alive: PSS and RSS of the whole tree once every worker has spoken."""
    env = dict(os.environ, KILN_WORKERS=str(n))
    proc = subprocess.Popen([binary, "hold"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
    seen, buffer = [], b""
    deadline = time.time() + 60
    try:
        while len(seen) < n and time.time() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], 0.5)
            if ready:
                chunk = os.read(proc.stdout.fileno(), 65536)
                if not chunk:
                    break
                buffer += chunk
                *lines, buffer = buffer.split(b"\n")
                seen += [int(line.split()[1]) for line in lines if line.startswith(b"ready ")]
        if len(seen) < n:
            return [{"runtime": "kiln", "case": f"proc-mem-{n}", "error": f"only {len(seen)} of {n} workers spoke"}]
        time.sleep(0.2)
        pss, rss, count = tree_memory(proc.pid)
        return [{"runtime": "kiln", "case": f"proc-mem-{n}", "pss_kib": pss, "rss_kib": rss,
                 "processes": count, "checksum": str(sum(i + 1 for i in seen))}]
    finally:
        pids = kill_tree(proc.pid)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            for pid in pids:
                try:
                    os.kill(pid, signal.SIGKILL)
                except OSError:
                    pass
            proc.wait()


def server(runtime, argv, env, seconds, connections):
    if port_open(PORT):
        return [{"runtime": runtime, "case": "server", "error": f"port {PORT} already in use"}]
    proc = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env, cwd=REPO,
                            start_new_session=True)
    try:
        deadline = time.time() + 20
        while time.time() < deadline and not http_ok(PORT):
            if proc.poll() is not None:
                break
            time.sleep(0.05)
        if not http_ok(PORT):
            err = proc.stderr.read(400).decode("utf-8", "replace") if proc.poll() is not None else ""
            return [{"runtime": runtime, "case": "server", "error": f"never answered: {err}"}]
        time.sleep(0.3)  # let every worker of a multi-process server bind
        url = f"http://127.0.0.1:{PORT}/"
        subprocess.run(["oha", "-z", "1s", "-c", str(connections), "--no-tui", "--output-format", "quiet", url],
                       capture_output=True)
        done = subprocess.run(["oha", "-z", f"{seconds}s", "-c", str(connections), "--no-tui",
                               "--output-format", "json", url], capture_output=True, text=True)
        pss, rss, count = tree_memory(proc.pid)
        data = json.loads(done.stdout)
        codes = data.get("statusCodeDistribution", {})
        ok = codes.get("200", 0)
        total = sum(codes.values())
        return [{"runtime": runtime, "case": "server", "rps": data["summary"]["requestsPerSec"],
                 "p50_ms": data["latencyPercentiles"]["p50"] * 1000,
                 "p99_ms": data["latencyPercentiles"]["p99"] * 1000,
                 # oha cancels the requests in flight when -z expires and counts them as errors;
                 # those are not the server's. Anything else is.
                 "http_errors": total - ok + sum(v for e, v in data.get("errorDistribution", {}).items()
                                                 if "deadline" not in e.lower()),
                 "pss_kib": pss, "rss_kib": rss, "processes": count, "checksum": "200"}]
    finally:
        pids = kill_tree(proc.pid)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        for pid in pids:
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass
        for _ in range(100):
            if not port_open(PORT):
                break
            time.sleep(0.05)


def jobs_for(cfg, bins):
    """Every job, grouped by workload. A job is (workload, runtime, thunk)."""
    go = bins.get("go")
    k = bins
    nproc = os.cpu_count()
    jobs = []

    def add(workload, runtime, thunk, needs=True):
        if needs:
            jobs.append((workload, runtime, thunk))

    # 1. start
    for rt in ("node", "bun"):
        add("start", rt, lambda rt=rt: simple(rt, js(rt, "start.mjs", "start", cfg["start_warm"])))
        for n in cfg["mem_ns"]:
            add("start", rt, lambda rt=rt, n=n: simple(rt, js(rt, "start.mjs", "mem", n)))
        add("start", rt, lambda rt=rt: process_start(rt, js(rt, "kiln-start.mjs"), cfg["kiln_reps"]))
    add("start", "go", lambda: simple("go", [go, "start", str(cfg["start_warm"])]), go)
    for n in cfg["mem_ns"]:
        add("start", "go", lambda n=n: simple("go", [go, "mem", str(n)]), go)
    # `node:worker_threads` in kiln (realms issue 06): the same program node and bun run, built.
    add("start", "kiln", lambda: simple("kiln", [k["start"], "start", str(cfg["start_warm"])]), k["start"])
    add("start", "kiln", lambda: simple("kiln", [k["actor-start"], str(cfg["start_warm"])]), k["actor-start"])
    add("start", "kiln", lambda: actor_capacity(k["actor-capacity"], cfg["actor_ns"]),
        k["actor-capacity"])
    for n in cfg["mem_ns"]:
        add("start", "kiln", lambda n=n: simple("kiln", [k["start"], "mem", str(n)]), k["start"])
    for n in cfg["kiln_ns"]:
        add("start", "kiln", lambda n=n: kiln_process_start(k["kiln-start"], n, cfg["kiln_reps"]), k["kiln-start"])
    for n in cfg["kiln_ns"]:
        add("start", "kiln", lambda n=n: kiln_process_memory(k["kiln-start"], n), k["kiln-start"])

    # 2. messages
    for rt in ("node", "bun"):
        add("messages", rt, lambda rt=rt: [dict(r, scope="thread") for r in
                                           simple(rt, js(rt, "messages.mjs", cfg["msg_budget"], cfg["oneway"]))])
        add("messages", rt, lambda rt=rt: [dict(r, scope="realm") for r in
                                           simple(rt, js(rt, "inrealm.mjs", cfg["msg_budget"], cfg["inrealm_oneway"]))])
    add("messages", "go", lambda: [dict(r, scope="thread") for r in simple("go", [go, "messages", str(cfg["msg_budget"])])], go)
    add("messages", "kiln", lambda: [dict(r, scope="thread") for r in
                                     simple("kiln", [k["messages"], str(cfg["msg_budget"]), str(cfg["oneway"])], timeout=300)],
        k["messages"])
    add("messages", "kiln", lambda: [dict(r, scope="realm") for r in
                                     simple("kiln", [k["inrealm"], str(cfg["msg_budget"]), str(cfg["inrealm_oneway"])], timeout=300)],
        k["inrealm"])
    add("messages", "kiln", lambda: simple(
        "kiln", [k["actor-graph"], str(cfg["actor_graph_samples"])], timeout=300),
        k["actor-graph"])

    # 3. shared dataset
    for n in cfg["shared_ns"]:
        for rt in ("node", "bun"):
            for mode in ("copy", "sab"):
                add("shared", rt, lambda rt=rt, n=n, mode=mode: simple(rt, js(rt, "shared.mjs", n, cfg["queries"], mode)))
        add("shared", "go", lambda n=n: simple("go", [go, "shared", str(n), str(cfg["queries"])]), go)
        for mode in ("copy", "sab"):
            add("shared", "kiln", lambda n=n, mode=mode: simple(
                "kiln", [k["shared"], str(n), str(cfg["queries"]), mode], timeout=300), k["shared"])

    # 4. many tasks
    for n in cfg["task_ns"]:
        targs = (n, cfg["ticks"], cfg["interval"])
        for rt in ("node", "bun"):
            add("tasks", rt, lambda rt=rt, targs=targs: simple(rt, js(rt, "tasks.mjs", *targs)))
        add("tasks", "go", lambda targs=targs: simple("go", [go, "tasks", *map(str, targs)]), go)
        add("tasks", "kiln", lambda targs=targs: simple("kiln", [k["tasks"], *map(str, targs)]), k["tasks"])

    # Each task is a separate native realm. This is intentionally distinct from the one-realm
    # async-task rows above; the launcher records one result per realm and the runner verifies all
    # of them. Large counts are omitted when their measured memory slope exceeds the host budget.
    add("realms", "kiln", lambda: realm_scaling(k["realm-periodic"], cfg["realm_ns"]),
        k["realm-periodic"])

    # 5. data-parallel map
    pargs = (cfg["pmap_n"], cfg["pmap_reps"])
    for rt in ("node", "bun"):
        add("pmap", rt, lambda rt=rt: simple(rt, js(rt, "pmap.mjs", *pargs)))
        add("pmap", rt, lambda rt=rt: simple(rt, js(rt, "pmap-pool.mjs", *pargs, nproc)))
        add("pmap", rt, lambda rt=rt: simple(rt, js(rt, "pmap.mjs", *pargs, "light")))
    add("pmap", "go", lambda: simple("go", [go, "pmap", *map(str, pargs)]), go)
    add("pmap", "go", lambda: simple("go", [go, "pmap", *map(str, pargs), "light"]), go)
    add("pmap", "kiln", lambda: simple("kiln", [k["pmap"], *map(str, pargs)]), k["pmap"])
    add("pmap", "kiln", lambda: simple("kiln", [k["pmap"], *map(str, pargs), "light"]), k["pmap"])
    add("pmap", "kiln", lambda: simple("kiln", [k["pmap-parallel"], *map(str, pargs)]), k["pmap-parallel"])
    # The same with the provider's pool held to one thread: if this matches the row above, the
    # parallel lowering did not run.
    add("pmap", "kiln", lambda: [dict(r, case=r.get("case", "?") + "-p1") for r in
                                 simple("kiln", [k["pmap-parallel"], *map(str, pargs)],
                                        env=dict(os.environ, KILN_PARALLELISM="1"))], k["pmap-parallel"])
    add("pmap", "kiln", lambda: simple("kiln", [k["pmap-parallel-light"], *map(str, pargs)]), k["pmap-parallel-light"])

    # 6. server
    s, c = cfg["server_s"], cfg["server_c"]
    env = dict(os.environ)
    add("server", "kiln", lambda: [dict(r, case="server-all") for r in
                                   server("kiln", [k["serve"]], dict(env, KILN_WORKERS="auto"), s, c)], k["serve"])
    add("server", "kiln", lambda: [dict(r, case="server-thread") for r in
                                   server("kiln", [k["serve"]],
                                          dict(env, KILN_WORKERS="auto", KILN_WORKERS_MODE="thread"), s, c)],
        k["serve"])
    add("server", "kiln", lambda: [dict(r, case="server-1") for r in
                                   server("kiln", [k["serve"]], dict(env, KILN_WORKERS="1"), s, c)], k["serve"])
    add("server", "node", lambda: [dict(r, case="server-all") for r in
                                   server("node", ["node", os.path.join(JS, "cluster.mjs")], env, s, c)])
    add("server", "node", lambda: [dict(r, case="server-1") for r in
                                   server("node", ["node", os.path.join(JS, "serve.mjs")], env, s, c)])
    add("server", "bun", lambda: [dict(r, case="server-all") for r in
                                  server("bun", ["bun", os.path.join(JS, "bun-serve.mjs")], env, s, c)])
    add("server", "go", lambda: [dict(r, case="server-all") for r in
                                 server("go", [go, "serve", str(PORT)], dict(env), s, c)], go)
    return jobs


# ------------------------------------------------------------------------------------------------
# aggregation


def key(record):
    return (record.get("case"), record.get("scope", ""), record["runtime"])


def aggregate(results):
    """{(case, scope, runtime): {metric: (median, spread), 'checksum': set, 'error': str}}"""
    grouped = {}
    for record in results:
        if "case" not in record:
            record["case"] = "?"
        grouped.setdefault(key(record), []).append(record)
    table = {}
    for k, records in grouped.items():
        cell = {"checksums": set(), "errors": [r["error"] for r in records if "error" in r]}
        good = [r for r in records if "error" not in r]
        for r in good:
            cell["checksums"].add(r.get("checksum"))
        metrics = {m for r in good for m, v in r.items() if isinstance(v, (int, float)) and not isinstance(v, bool)}
        for m in metrics:
            values = [r[m] for r in good if isinstance(r.get(m), (int, float))]
            if not values:
                continue
            mid = statistics.median(values)
            spread = (max(values) - min(values)) / mid if mid else 0.0
            cell[m] = (mid, spread, len(values))
        table[k] = cell
    return table


REFERENCE = ["go", "node"]


def divergences(table):
    """Cells whose checksum differs from the reference runtime's for the same case and scope."""
    out = {}
    cases = {(c, s) for (c, s, _) in table}
    for case, scope in cases:
        ref = None
        for runtime in REFERENCE:
            cell = table.get((case, scope, runtime))
            if cell and len(cell["checksums"]) == 1 and not cell["errors"]:
                ref = (runtime, next(iter(cell["checksums"])))
                break
        for (c, s, runtime), cell in table.items():
            if (c, s) != (case, scope):
                continue
            if len(cell["checksums"]) > 1:
                out[(c, s, runtime)] = f"checksum changed between rounds: {sorted(map(str, cell['checksums']))}"
            elif ref and cell["checksums"] and next(iter(cell["checksums"])) != ref[1]:
                out[(c, s, runtime)] = f"checksum {next(iter(cell['checksums']))} != {ref[0]}'s {ref[1]}"
    return out


# ------------------------------------------------------------------------------------------------
# report


def fmt_num(value, unit=""):
    if value is None:
        return "--"
    if float(value).is_integer() and abs(value) < 100000:
        return f"{int(value):,}" + unit
    if abs(value) >= 100000:
        text = f"{value / 1000:,.0f}k"
    elif abs(value) >= 1000:
        text = f"{value:,.0f}"
    elif abs(value) >= 100:
        text = f"{value:.0f}"
    elif abs(value) >= 10:
        text = f"{value:.1f}"
    elif abs(value) >= 1:
        text = f"{value:.2f}"
    else:
        text = f"{value:.3g}"
    return text + unit


def mb(kib):
    return None if kib is None else kib / 1024


class Report:
    def __init__(self, table, diverged, rounds):
        self.table, self.diverged, self.rounds = table, diverged, rounds
        self.noisy_cells = 0

    def cell(self, case, runtime, metric, unit="", scale=None, scope=""):
        k = (case, scope, runtime)
        c = self.table.get(k)
        if c is None:
            return None
        if k in self.diverged:
            return "excluded (divergence)"
        if c["errors"] and metric not in c:
            reason = c["errors"][0].splitlines()[0][:80].replace("|", "/")
            if reason.startswith("skipped:"):
                return reason if metric == "wall_ms" else "--"
            return "failed: " + reason
        if metric not in c:
            return "--"
        mid, spread, n = c[metric]
        value = scale(mid) if scale else mid
        text = fmt_num(value, unit)
        if n > 1 and spread > NOISY:
            self.noisy_cells += 1
            text = "~" + text
        return text

    def row(self, label, cells):
        return "| " + label + " | " + " | ".join(c if c is not None else "--" for c in cells) + " |"


def render(table, diverged, failures, machine, cfg, loads, started, finished, quick, ran):
    rep = Report(table, diverged, cfg["rounds"])
    L = []
    w = L.append
    na_kiln_worker = "not measured"
    w(f"# Concurrency benchmarks, {started[:10]}")
    w("")
    w(f"Produced by `bench/concurrency/run.sh{' --quick' if quick else ''}`; see `bench/concurrency/README.md` for what each row "
      "measures and what \"not available\" means. Medians of "
      f"{cfg['rounds']} interleaved round(s); a leading `~` marks a cell whose rounds spread by more than "
      f"{int(NOISY * 100)}% of the median. Lower is better except where the unit is a rate (/s).")
    w("")
    w("## Machine")
    w("")
    for k, v in machine.items():
        w(f"- **{k}**: {v}")
    w(f"- **started / finished**: {started} / {finished}")
    w(f"- **load average (1 min) seen before each job**: min {min(loads):.1f}, median {statistics.median(loads):.1f}, "
      f"max {max(loads):.1f} on {os.cpu_count()} hardware threads")
    w("")

    # 1. start
    w("## 1. Worker start")
    w("")
    w("| | node `worker_threads` | bun `Worker` | Go goroutine + channel | kiln `worker_threads` |")
    w("| --- | --- | --- | --- | --- |")
    w(rep.row("start one worker, first message back: warm median", [
        rep.cell("start", "node", "median_ms", " ms"), rep.cell("start", "bun", "median_ms", " ms"),
        rep.cell("start", "go", "median_ms", " µs", scale=lambda v: v * 1000),
        rep.cell("start", "kiln", "median_ms", " ms") or na_kiln_worker]))
    w(rep.row("the same, first (cold) start", [
        rep.cell("start", "node", "cold_ms", " ms"), rep.cell("start", "bun", "cold_ms", " ms"),
        rep.cell("start", "go", "cold_ms", " µs", scale=lambda v: v * 1000),
        rep.cell("start", "kiln", "cold_ms", " ms") or na_kiln_worker]))
    for n in cfg["mem_ns"]:
        w(rep.row(f"memory per added worker, {n} alive (RSS delta / N)", [
            rep.cell(f"mem-{n}", "node", "per_worker_kib", " MB", scale=mb),
            rep.cell(f"mem-{n}", "bun", "per_worker_kib", " MB", scale=mb),
            rep.cell(f"mem-{n}", "go", "per_worker_kib", " KB"),
            rep.cell(f"mem-{n}", "kiln", "per_worker_kib", " MB", scale=mb) or na_kiln_worker]))
    w("")
    w("The public `spawnActor` path is measured separately because its ready handshake and first method call add work beyond `new Worker`. Each sample creates and stops a fresh actor realm; process startup is outside the clock.")
    w("")
    w("| kiln actor API | time |")
    w("| --- | ---: |")
    w(rep.row("`spawnActor` to ready, first (cold)", [rep.cell("actor-start", "kiln", "cold_ms", " ms")]))
    w(rep.row("`spawnActor` to ready, warm median", [rep.cell("actor-start", "kiln", "median_ms", " ms")]))
    w(rep.row("first method call after ready, warm median", [rep.cell("actor-start", "kiln", "first_call_ms", " ms")]))
    w("")
    w("Public actor capacity keeps every spawned realm alive while calling each one. The request window is 64; held RSS and thread count are sampled after all actors are ready. Every reply and stop is checked. The 10,000-actor row runs only when the smaller counts project a safe memory budget.")
    w("")
    w("| actors alive | spawn to all ready | one call each | stop all | held RSS | held threads |")
    w("| ---: | ---: | ---: | ---: | ---: | ---: |")
    for n in cfg["actor_ns"]:
        w(rep.row(f"{n:,}", [
            rep.cell(f"actor-capacity-{n}", "kiln", "spawn_ms", " ms"),
            rep.cell(f"actor-capacity-{n}", "kiln", "calls_ms", " ms"),
            rep.cell(f"actor-capacity-{n}", "kiln", "stop_ms", " ms"),
            rep.cell(f"actor-capacity-{n}", "kiln", "held_rss_kib", " MB", scale=mb),
            rep.cell(f"actor-capacity-{n}", "kiln", "held_threads", "")]))
    w("")
    w("Deployment comparison: one `KILN_WORKERS` process per worker (process mode, forked by the "
      "supervisor from a process that never built a realm). Timed from spawn to exit of the whole tree, each "
      "worker printing one line (the \"message back\"); memory is the summed PSS of the held-alive tree. The "
      "node and bun columns time the same one-line program as a whole process, for context.")
    w("")
    w("| process start (spawn -> line -> exit), warm median | node | bun | kiln |")
    w("| --- | --- | --- | --- |")
    w(rep.row("1 process", [rep.cell("proc-start-1", "node", "median_ms", " ms"),
                            rep.cell("proc-start-1", "bun", "median_ms", " ms"),
                            (rep.cell("proc-start-1", "kiln", "median_ms", " ms") or "--") + " (`KILN_WORKERS=1`, no supervisor)"]))
    w("")
    w("| `KILN_WORKERS=N` | wall, spawn to all N lines and exit | tree PSS held alive | tree RSS (sums shared text N times) | PSS per added worker |")
    w("| --- | --- | --- | --- | --- |")
    base = table.get(("proc-mem-1", "", "kiln"), {}).get("pss_kib")
    for n in cfg["kiln_ns"]:
        mem = table.get((f"proc-mem-{n}", "", "kiln"), {})
        per = "--"
        if n > 1 and base and "pss_kib" in mem:
            per = fmt_num((mem["pss_kib"][0] - base[0]) / (n - 1) / 1024, " MB")
        w(rep.row(f"N = {n}", [rep.cell(f"proc-start-{n}", "kiln", "median_ms", " ms"),
                               rep.cell(f"proc-mem-{n}", "kiln", "pss_kib", " MB", scale=mb) or "--",
                               rep.cell(f"proc-mem-{n}", "kiln", "rss_kib", " MB", scale=mb) or "--", per]))
    w("")

    # 2. messages
    w("## 2. Messages")
    w("")
    w("Across threads (a `worker_threads` Worker; Go: a goroutine; kiln: a `worker_threads` Worker, a realm on "
      "a thread of the realm host, realms issue 06).")
    w("")
    w("| | node | bun | Go | kiln |")
    w("| --- | --- | --- | --- | --- |")
    msg_rows = [
        ("small object, ping-pong round trip", "small-pingpong", "us_per_op", " µs"),
        ("small object, one-way throughput", "small-oneway", "msgs_per_s", "/s"),
        ("1 MB object graph, send + ack (Go: deep copy)", "graph-1mb", "ms_per_op", " ms"),
        ("16 MB ArrayBuffer, round trip, copied", "ab16-copy", "ms_per_op", " ms"),
        ("16 MB ArrayBuffer, round trip, transferred", "ab16-transfer", "ms_per_op", " ms"),
    ]
    for label, case, metric, unit in msg_rows:
        w(rep.row(label, [rep.cell(case, rt, metric, unit, scope="thread") for rt in ("node", "bun", "go", "kiln")]))
    w(rep.row("1 MB object graph by reference (Go only)", ["--", "--",
                                                           rep.cell("graph-1mb-ref", "go", "ms_per_op", " ms", scope="thread"), "--"]))
    w("")
    w("In one realm: the global `MessageChannel`, both ports on the main thread (structured clone plus an "
      f"event-loop hop, no thread crossing). The one-way row sends {cfg['inrealm_oneway']:,} messages here "
      f"({cfg['oneway']:,} across threads).")
    w("")
    w("| in-realm `MessageChannel` | node | bun | kiln |")
    w("| --- | --- | --- | --- |")
    for label, case, metric, unit in msg_rows:
        w(rep.row(label.replace(" (Go: deep copy)", ""), [rep.cell(case, rt, metric, unit, scope="realm") for rt in ("node", "bun", "kiln")]))
    w("")
    w("The public actor call uses the same 1 MB graph in both modes. `prepareTransfer` makes a "
      "private snapshot on the sender; its preparation time is included in the total. The actor "
      "computes the same checksum for both calls. Actor startup is outside these clocks.")
    w("")
    w("| kiln actor graph call | time |")
    w("| --- | ---: |")
    for label, case in [
        ("structured clone call", "actor-graph-copy"),
        ("structured clone round trip", "actor-graph-roundtrip-copy"),
        ("prepare private snapshot", "actor-graph-prepare"),
        ("prepared handoff and reply", "actor-graph-handoff"),
        ("prepared total", "actor-graph-prepared-total"),
    ]:
        w(rep.row(label, [rep.cell(case, "kiln", "ms_per_op", " ms")]))
    w("")

    # 3. shared
    w("## 3. A 50 MB read-only table shared by N workers")
    w("")
    w(f"Each worker does {cfg['queries']:,} pseudo-random lookups. Setup: from the first send until every "
      "worker holds the table (workers already started). Query: all workers' lookups, median of 3. RSS: the "
      "whole process after setup. The SharedArrayBuffer row shares bytes across workers; "
      "this row measures shared bytes; the frozen-value API (issue 11) has its own semantics.")
    w("")
    w("| N | runtime / mode | setup | query | RSS |")
    w("| --- | --- | --- | --- | --- |")
    for n in cfg["shared_ns"]:
        for rt, mode, case in [("node", "copy (structured clone per worker)", f"shared-{n}-copy"),
                               ("node", "SharedArrayBuffer", f"shared-{n}-sab"),
                               ("bun", "copy (structured clone per worker)", f"shared-{n}-copy"),
                               ("bun", "SharedArrayBuffer", f"shared-{n}-sab"),
                               ("go", "one slice, shared", f"shared-{n}"),
                               ("kiln", "copy (structured clone per worker)", f"shared-{n}-copy"),
                               ("kiln", "SharedArrayBuffer", f"shared-{n}-sab")]:
            w(rep.row(str(n), [f"{rt}: {mode}", rep.cell(case, rt, "setup_ms", " ms"),
                               rep.cell(case, rt, "query_ms", " ms"),
                               rep.cell(case, rt, "rss_kib", " MB", scale=mb)]))
    w("")

    # 4. tasks
    w("## 4. Many concurrent tasks")
    w("")
    w(f"N tasks each wake {cfg['ticks']} times on a {cfg['interval']} ms timer and add to a counter (floor: "
      f"{cfg['ticks'] * cfg['interval']} ms). JavaScript: async functions in one realm, kiln included (one realm; "
      "the separate native-realm workload is section 7). Go: goroutines. Peak RSS is the process's maximum resident set.")
    w("")
    w("| N | metric | node | bun | Go | kiln (async tasks, one realm) |")
    w("| --- | --- | --- | --- | --- | --- |")
    for n in cfg["task_ns"]:
        case = f"tasks-{n}"
        w(rep.row(f"{n:,}", ["wall"] + [rep.cell(case, rt, "wall_ms", " ms") for rt in ("node", "bun", "go", "kiln")]))
        w(rep.row(f"{n:,}", ["ticks/s"] + [rep.cell(case, rt, "ticks_per_s", "/s") for rt in ("node", "bun", "go", "kiln")]))
        w(rep.row(f"{n:,}", ["peak RSS"] + [rep.cell(case, rt, "maxrss_kib", " MB", scale=mb) for rt in ("node", "bun", "go", "kiln")]))
    w("")
    w("These JavaScript tasks share one realm. Section 7 measures separate native realms and "
      "includes the launcher's result-file cost.")
    w("")

    # 5. pmap
    w("## 5. Data-parallel map")
    w("")
    w(f"`(x) => {{ 16 rounds of y = (y * 31 + k) % 1000003 }}` over {cfg['pmap_n']:,} numbers; warm median "
      f"of {cfg['pmap_reps']} (after one cold run). Peak RSS of the whole run.")
    w("")
    w("| | node | bun | Go | kiln |")
    w("| --- | --- | --- | --- | --- |")
    pm_fail = failures.get("pmap-parallel")
    w(rep.row("sequential `map` (Go: a loop)", [rep.cell("pmap-map", rt, "median_ms", " ms") for rt in ("node", "bun", "go", "kiln")]))
    w(rep.row(f"all cores (node/bun: `worker_threads` pool of {os.cpu_count()}, copy in, transfer back; Go: goroutines; kiln: `parallelMap`)", [
        rep.cell("pmap-pool", "node", "median_ms", " ms"), rep.cell("pmap-pool", "bun", "median_ms", " ms"),
        rep.cell("pmap-parallel", "go", "median_ms", " ms"),
        rep.cell("pmap-parallel", "kiln", "median_ms", " ms") if not pm_fail else "build failed (see caveats)"]))
    w(rep.row("kiln `parallelMap` with `KILN_PARALLELISM=1`", ["--", "--", "--",
              rep.cell("pmap-parallel-p1", "kiln", "median_ms", " ms") if not pm_fail else "build failed"]))
    w(rep.row("peak RSS, sequential", [rep.cell("pmap-map", rt, "maxrss_kib", " MB", scale=mb) for rt in ("node", "bun", "go", "kiln")]))
    w("")
    w("The same over a light callback, `(x) => (x * 3 + 1) % 1000003`: memory-bound, but the shape kiln's typed "
      "`parallelMap` pool accepts.")
    w("")
    w("| light callback | node | bun | Go | kiln |")
    w("| --- | --- | --- | --- | --- |")
    w(rep.row("sequential `map` (Go: a loop)", [rep.cell("pmap-light-map", rt, "median_ms", " ms") for rt in ("node", "bun", "go", "kiln")]))
    light_fail = failures.get("pmap-parallel-light")
    w(rep.row("all cores (Go: goroutines; kiln: `parallelMap`)", [
        "--", "--", rep.cell("pmap-light-parallel", "go", "median_ms", " ms"),
        rep.cell("pmap-light-parallel", "kiln", "median_ms", " ms") if not light_fail else "build failed (see failures)"]))
    w("")

    # 6. server
    w("## 6. Hello-world HTTP server at all cores")
    w("")
    w(f"`oha -c {cfg['server_c']} -z {cfg['server_s']}s` after a 1 s warm-up, on the same machine as the server "
      "(the load generator competes for the same cores). Memory: summed PSS (and RSS) of the server's process "
      "tree after the load.")
    w("")
    w("| server | rps | p50 | p99 | tree PSS | tree RSS | processes | errors |")
    w("| --- | --- | --- | --- | --- | --- | --- | --- |")
    for label, case, rt in [("kiln `KILN_WORKERS=auto` (supervisor + one realm process per core)", "server-all", "kiln"),
                            ("kiln `KILN_WORKERS_MODE=thread` (one process, one realm per core)", "server-thread", "kiln"),
                            ("node `cluster` (primary + one worker per core)", "server-all", "node"),
                            ("bun `Bun.serve` + `reusePort`, one process per core", "server-all", "bun"),
                            ("Go `net/http`", "server-all", "go"),
                            ("kiln `KILN_WORKERS=1` (one realm, for scale)", "server-1", "kiln"),
                            ("node, one process (for scale)", "server-1", "node")]:
        w(rep.row(label, [rep.cell(case, rt, "rps", "/s"), rep.cell(case, rt, "p50_ms", " ms"),
                          rep.cell(case, rt, "p99_ms", " ms"), rep.cell(case, rt, "pss_kib", " MB", scale=mb),
                          rep.cell(case, rt, "rss_kib", " MB", scale=mb), rep.cell(case, rt, "processes"),
                          rep.cell(case, rt, "http_errors")]))
    w("")
    w("This is a high-concurrency, same-machine HTTP measurement. It does not remeasure the "
      "16-connection case where thread mode previously trailed process mode, or the value of "
      "process mode's crash isolation; process remains the default (issue 07).")
    w("")

    # 7. one native realm per task
    w("## 7. Separate native realms on the turn scheduler")
    w("")
    w("Each realm wakes on a timer 10 times and prints `10`; every output and exit status is "
      "checked. Wall time includes process launch, realm setup, timer turns, and the native "
      "launcher's two result files per realm. Peak RSS is for the whole process. The 100,000 "
      "realm row runs only if the RSS slope measured at smaller counts fits within 70% of "
      "available memory with a 1.5× margin. This row has no Node/Bun equivalent: their "
      "one-realm async tasks appear in section 4, and spawning 100,000 worker isolates is "
      "a different resource budget.")
    w("")
    w("| realms | wall | turns/s | peak RSS |")
    w("| ---: | ---: | ---: | ---: |")
    for n in cfg["realm_ns"]:
        case = f"realms-{n}"
        w(rep.row(f"{n:,}", [rep.cell(case, "kiln", "wall_ms", " ms"),
                              rep.cell(case, "kiln", "turns_per_s", "/s"),
                              rep.cell(case, "kiln", "maxrss_kib", " MB", scale=mb)]))
    w("")

    w("## Divergences, failures and skips")
    w("")
    if not diverged and not failures and not any(c["errors"] for c in table.values()):
        if ran == {"realms"}:
            w("None: every native realm produced the expected output and exit status.")
        else:
            w("None: every comparable answer matched the reference runtime (Go where it runs the case, else node).")
    for (case, scope, runtime), why in sorted(diverged.items()):
        w(f"- **excluded** {runtime} `{case}`{' (' + scope + ')' if scope else ''}: {why}")
    for name, why in sorted(failures.items()):
        what = "the Go build failed" if name == "go" else f"kiln build of `{name}.mjs` failed"
        w(f"- **{what}**: `{why.splitlines()[0] if why else '?'}`")
    for (case, scope, runtime), cell in sorted(table.items(), key=lambda kv: str(kv[0])):
        for err in [e for e in cell["errors"] if isinstance(e, str)][:1]:
            outcome = "skipped" if err.startswith("skipped:") else "failed"
            w(f"- **{runtime} `{case}` {outcome}** in {len(cell['errors'])} round(s): `{err.splitlines()[0][:200]}`")
    w("")
    w(f"Noisy cells (rounds spread > {int(NOISY * 100)}%): {rep.noisy_cells}.")
    # Leave out the sections of workloads this run did not include (--only).
    numbered = {"1": "start", "2": "messages", "3": "shared", "4": "tasks", "5": "pmap", "6": "server", "7": "realms"}
    sections = "\n".join(L).split("\n## ")
    kept = [sections[0]] + [sec for sec in sections[1:] if numbered.get(sec.split(".", 1)[0], "") in ran or not sec[0].isdigit()]
    return "\n## ".join(kept) + "\n"


# ------------------------------------------------------------------------------------------------


def sh(argv):
    try:
        return subprocess.run(argv, capture_output=True, text=True, cwd=REPO).stdout.strip()
    except OSError:
        return "?"


def machine_info(kiln, go_cmd):
    cpu = "?"
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                cpu = line.split(":", 1)[1].strip()
                break
    except OSError:
        pass
    mem = "?"
    try:
        for line in open("/proc/meminfo"):
            if line.startswith("MemTotal"):
                mem = f"{int(line.split()[1]) / 1024 / 1024:.1f} GiB"
    except OSError:
        pass
    commit = sh(["git", "rev-parse", "--short", "HEAD"])
    dirty = sh(["git", "status", "--porcelain", "--untracked-files=no"])
    return {
        "kernel": " ".join(platform.uname()[i] for i in (0, 2, 4)),
        "cpu": f"{cpu}, {os.cpu_count()} hardware threads",
        "memory": mem,
        "kiln": f"{commit}{' (dirty)' if dirty else ''}, `{os.path.relpath(kiln, REPO)}`, native binaries `--profile native-release`, "
                + sh(["clang", "--version"]).splitlines()[0],
        "node": sh(["node", "--version"]),
        "bun": sh(["bun", "--version"]),
        "go": sh(go_cmd + ["version"]).replace("go version ", "") if go_cmd else "not used",
        "oha": sh(["oha", "--version"]),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--out", help="results markdown (default: bench/concurrency/results-<date>.md)")
    parser.add_argument("--only", help="comma-separated workloads: start,messages,shared,tasks,pmap,server,realms")
    parser.add_argument("--kiln", default=os.path.join(REPO, "target", "release", "kiln"))
    parser.add_argument("--go", default=os.environ.get("GO_CMD", ""))
    args = parser.parse_args()
    selected = set(args.only.split(",")) if args.only else {"start", "messages", "shared", "tasks", "pmap", "server", "realms"}
    unknown = selected - {"start", "messages", "shared", "tasks", "pmap", "server", "realms"}
    if unknown:
        parser.error("unknown workload(s): " + ", ".join(sorted(unknown)))
    cfg = config(args.quick)
    go_cmd = args.go.split() if args.go else None
    if go_cmd is None:
        if selected == {"realms"}:
            go_cmd = []
        else:
            # A mise shim can be on PATH and still refuse to run (no version set), so ask it.
            works = shutil.which("go") and subprocess.run(["go", "version"], capture_output=True).returncode == 0
            go_cmd = ["go"] if works else ["mise", "exec", "go@latest", "--", "go"]

    started = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
    work = os.path.join(REPO, "target", "bench-concurrency", started.replace(":", ""))
    os.makedirs(work, exist_ok=True)
    log(f"bench/concurrency: building into {os.path.relpath(work, REPO)}")
    bins, failures = build_all(args.kiln, go_cmd, work, selected)
    machine = machine_info(args.kiln, go_cmd)

    jobs = jobs_for(cfg, bins)
    if args.only:
        wanted = set(args.only.split(","))
        jobs = [j for j in jobs if j[0] in wanted]
    results, loads = [], []
    workloads = []
    for wl, _, _ in jobs:
        if wl not in workloads:
            workloads.append(wl)
    for round_index in range(cfg["rounds"]):
        for wl in workloads:
            group = [j for j in jobs if j[0] == wl]
            runtimes = sorted({rt for _, rt, _ in group})
            shift = round_index % len(runtimes)
            order = runtimes[shift:] + runtimes[:shift]
            for rt in order:
                for _, jrt, thunk in group:
                    if jrt != rt:
                        continue
                    loads.append(os.getloadavg()[0])
                    t = time.time()
                    records = thunk()
                    for r in records:
                        r["round"] = round_index
                        r["workload"] = wl
                        r["load1"] = loads[-1]
                    results.extend(records)
                    brief = ", ".join(str(r.get("case", "?")) + ("!" if "error" in r else "") for r in records)
                    log(f"  round {round_index + 1} {wl:<8} {rt:<4} {time.time() - t:6.1f}s  load {loads[-1]:.1f}  {brief}")
    finished = datetime.datetime.now().astimezone().isoformat(timespec="seconds")

    raw = os.path.join(work, "raw.json")
    with open(raw, "w") as handle:
        json.dump({"machine": machine, "config": cfg, "results": results, "build_failures": failures}, handle, indent=1)
    table = aggregate(results)
    diverged = divergences(table)
    text = render(table, diverged, failures, machine, cfg, loads or [0.0], started, finished, args.quick, set(workloads))
    out = args.out or os.path.join(HERE, f"results-{started[:10]}.md")
    with open(out, "w") as handle:
        handle.write(text)
    log(f"bench/concurrency: wrote {os.path.relpath(out, REPO)} (raw: {os.path.relpath(raw, REPO)})")
    print(text)


if __name__ == "__main__":
    main()
