#!/usr/bin/env python3
"""One live realm or worker per loopback TCP connection (Linux /proc metrics)."""

import argparse
import asyncio
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time


HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


async def reply(reader, writer):
    try:
        data = await reader.readexactly(4)
        writer.write(b"pong" if data == b"ping" else b"wrong")
        await writer.drain()
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
                   KILN_REALM_MIGRATE="1", KILN_PARALLELISM=str(lanes))
        command = [str(binary)]
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
        return dict(runtime=kind, realms=count, wall_ms=round(elapsed, 2),
                    peak_threads=threads, peak_rss_kib=rss, checksum=count)
    finally:
        if output_dir is not None:
            output_dir.cleanup()
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kiln", type=Path, default=REPO / "target/release/kiln")
    parser.add_argument("--binary", type=Path, help="existing native client binary")
    parser.add_argument("--lanes", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--quick", action="store_true", help="one round, 1 and 100 clients")
    args = parser.parse_args()
    if args.lanes < 1:
        parser.error("--lanes must be positive")
    work = REPO / "target/bench-concurrency"
    work.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve() if args.binary else work / "connection-client"
    if not args.binary:
        build_native(args.kiln.resolve(), binary)
    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    server = asyncio.run_coroutine_threadsafe(
        asyncio.start_server(reply, "127.0.0.1", 0), loop).result()
    port = server.sockets[0].getsockname()[1]
    records = []
    try:
        counts = (1, 100) if args.quick else (1, 10, 100, 500)
        for round_number in range(1 if args.quick else 3):
            kinds = ("node", "pinned", "scheduled")
            kinds = kinds[round_number:] + kinds[:round_number]
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
