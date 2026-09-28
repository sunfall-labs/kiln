#!/usr/bin/env python3
"""One live realm or worker per loopback TCP connection (Linux /proc metrics)."""

import argparse
import asyncio
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import threading
import time


HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SERVER_EVENTS = []
SERVER_LOCK = threading.Lock()


async def reply(reader, writer):
    try:
        accepted = time.monotonic()
        data = await reader.readexactly(4)
        writer.write(b"pong" if data == b"ping" else b"wrong")
        await writer.drain()
        with SERVER_LOCK:
            SERVER_EVENTS.append((accepted, time.monotonic()))
    finally:
        writer.close()
        await writer.wait_closed()


def build_native(kiln, output):
    command = [str(kiln), "--one-shot", "--node", "build",
               str(HERE / "js/connection-client.mjs"), "--profile", "native-release",
               "--output", str(output)]
    subprocess.run(command, check=True, cwd=REPO)


def measure(kind, count, port, binary, work, lanes):
    env = os.environ.copy()
    env["BENCH_PORT"] = str(port)
    env["BENCH_REALMS"] = str(count)
    output_dir = None
    if kind == "node":
        command = ["node", str(HERE / "js/connection-node.mjs")]
    else:
        output_dir = tempfile.TemporaryDirectory(prefix="realm-outputs-", dir=work)
        env.update(KILN_REALMS=str(count), KILN_REALMS_OUT=output_dir.name,
                   KILN_REALM_SCHEDULER="0" if kind == "pinned" else "1",
                   KILN_REALM_MIGRATE=os.environ.get("KILN_REALM_MIGRATE", "1"),
                   KILN_PARALLELISM=str(lanes))
        command = [str(binary)]
    with SERVER_LOCK:
        SERVER_EVENTS.clear()
    started = time.monotonic()
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE)
    threads = rss = 0
    try:
        while process.poll() is None:
            try:
                fields = {parts[0]: parts[1] for line in Path(
                    f"/proc/{process.pid}/status").read_text().splitlines()
                          if (parts := line.split()) and len(parts) > 1}
                threads = max(threads, int(fields.get("Threads:", 0)))
                rss = max(rss, int(fields.get("VmRSS:", 0)))
            except FileNotFoundError:
                pass
            time.sleep(0.002)
        stdout, stderr = process.communicate(timeout=30)
        elapsed = (time.monotonic() - started) * 1000
        if process.returncode:
            raise RuntimeError(f"{kind} {count}: exit {process.returncode}: {stderr.decode()[-500:]}")
        if kind == "node":
            good = stdout.decode().strip() == f"{count}/{count}"
        else:
            out = Path(output_dir.name)
            statuses = list(out.glob("*.status"))
            replies = list(out.glob("*.out"))
            good = (len(statuses) == len(replies) == count
                    and all(path.read_text().strip() == "0" for path in statuses)
                    and all(path.read_text().strip() == "ok" for path in replies))
        if not good:
            raise RuntimeError(f"{kind} {count}: wrong or missing replies: {stdout[-200:]} {stderr[-200:]}")
        with SERVER_LOCK:
            events = list(SERVER_EVENTS)
        if len(events) != count:
            raise RuntimeError(f"{kind} {count}: server handled {len(events)} connections")
        accept_ms = sorted((start - started) * 1000 for start, _ in events)
        response_ms = sorted((end - start) * 1000 for start, end in events)
        return dict(runtime=kind, realms=count, wall_ms=round(elapsed, 2),
                    first_accept_ms=round(accept_ms[0], 2),
                    median_accept_ms=round(statistics.median(accept_ms), 2),
                    last_accept_ms=round(accept_ms[-1], 2),
                    last_response_ms=round((max(end for _, end in events) - started) * 1000, 2),
                    response_span_ms=round((max(end for _, end in events)
                                            - min(start for start, _ in events)) * 1000, 2),
                    response_p50_ms=round(statistics.median(response_ms), 2),
                    response_p95_ms=round(response_ms[int((count - 1) * 0.95)], 2),
                    peak_threads=threads, peak_rss_kib=rss, checksum=count)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        if output_dir is not None:
            output_dir.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kiln", type=Path, default=REPO / "target/release/kiln")
    parser.add_argument("--binary", type=Path, help="existing native client binary")
    parser.add_argument("--lanes", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--backlog", type=int, default=4096,
                        help="loopback listen backlog; keep above the largest client count")
    parser.add_argument("--quick", action="store_true", help="one round, 1 and 100 clients")
    parser.add_argument("--counts", type=int, nargs="+", help="override client counts")
    parser.add_argument("--rounds", type=int, help="override interleaved round count")
    parser.add_argument("--runtimes", nargs="+", choices=("node", "pinned", "scheduled"),
                        help="restrict the runtime comparison")
    args = parser.parse_args()
    if args.lanes < 1:
        parser.error("--lanes must be positive")
    if args.backlog < 1:
        parser.error("--backlog must be positive")
    if args.rounds is not None and args.rounds < 1:
        parser.error("--rounds must be positive")
    if args.counts and any(count < 1 for count in args.counts):
        parser.error("--counts must be positive")
    work = REPO / "target/bench-concurrency"
    work.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve() if args.binary else work / "connection-client"
    if not args.binary:
        build_native(args.kiln.resolve(), binary)
    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    server = asyncio.run_coroutine_threadsafe(
        asyncio.start_server(reply, "127.0.0.1", 0, backlog=args.backlog), loop).result()
    port = server.sockets[0].getsockname()[1]
    records = []
    try:
        counts = args.counts or ((1, 100) if args.quick else (1, 10, 100, 500))
        rounds = args.rounds or (1 if args.quick else 3)
        for round_number in range(rounds):
            kinds = tuple(args.runtimes or ("node", "pinned", "scheduled"))
            kinds = kinds[round_number % len(kinds):] + kinds[:round_number % len(kinds)]
            for count in counts:
                for kind in kinds:
                    if kind == "node" and count > 100:
                        continue  # hundreds of OS workers are not the Node comparison of interest
                    record = measure(kind, count, port, binary, work, args.lanes)
                    record["round"] = round_number + 1
                    records.append(record)
                    print(json.dumps(record), flush=True)
    finally:
        server.close()
        asyncio.run_coroutine_threadsafe(server.wait_closed(), loop).result()
        loop.call_soon_threadsafe(loop.stop)
        thread.join()
    destination = work / "realm-connections.json"
    destination.write_text(json.dumps(records, indent=2) + "\n")
    print(f"raw results: {destination}")


if __name__ == "__main__":
    main()
