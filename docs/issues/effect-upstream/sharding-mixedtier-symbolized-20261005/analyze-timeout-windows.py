#!/usr/bin/env python3
"""Summarize native symbol leaves and adjacent passing controls per marker window."""
import bisect, collections, gzip, json, sys
from pathlib import Path

base = Path(__file__).parent
profile = json.load(gzip.open(base / "profile.json.gz", "rt"))
symbols_doc = json.load(gzip.open(base / "profile.json.syms.json.gz", "rt"))
image = next(item for item in symbols_doc["data"] if item["debug_name"] == "symbolized.bin")
symbols = sorted(image["symbol_table"], key=lambda item: item["rva"])
starts = [item["rva"] for item in symbols]
strings = symbols_doc["string_table"]
thread = next(item for item in profile["threads"] if item["name"] == "symbolized.bin")
frames, stacks, funcs = thread["frameTable"], thread["stackTable"], thread["funcTable"]
native_resource = next(
    index for index, library_index in enumerate(thread["resourceTable"]["lib"])
    if profile["libs"][library_index]["debugName"] == "symbolized.bin"
)

def frame_name(frame_index):
    address = frames["address"][frame_index]
    if address is not None and address >= 0:
        index = bisect.bisect_right(starts, address) - 1
        if index >= 0 and address < symbols[index]["rva"] + symbols[index]["size"]:
            return strings[symbols[index]["symbol"]]
    function = frames["func"][frame_index]
    return thread["stringArray"][funcs["name"][function]] if function is not None else "<unknown>"

samples = []
for index, leaf_stack in enumerate(thread["samples"]["stack"]):
    if leaf_stack is None:
        continue
    chain, stack_index = [], leaf_stack
    while stack_index is not None and stack_index >= 0:
        chain.append(frame_name(stacks["frame"][stack_index]))
        stack_index = stacks["prefix"][stack_index]
    leaf_frame = stacks["frame"][leaf_stack]
    leaf_function = frames["func"][leaf_frame]
    if funcs["resource"][leaf_function] == native_resource:
        samples.append((thread["samples"]["time"][index], chain))

events = []
with gzip.open(base / "marker-arrivals.jsonl.gz", "rt") as stream:
    for line in stream:
        marker = json.loads(line)
        record = marker["record"]
        if record.get("kind") in ("inventory", "case"):
            events.append((marker["monotonic_ns"] / 1_000_000, record))

windows, previous = [], None
for arrival, record in events:
    if record["kind"] == "inventory":
        previous = (arrival, record)
        continue
    if previous is None:
        previous = (arrival, record)
        continue
    chains = [chain for when, chain in samples if previous[0] <= when <= arrival]
    leaves = collections.Counter(chain[0] for chain in chains)
    inclusive = collections.Counter(name for chain in chains for name in set(chain))
    adjacent = collections.Counter(
        (chain[index], chain[index + 1])
        for chain in chains for index in range(len(chain) - 1)
    )
    windows.append({
        "id": record.get("id"), "outcome": record.get("outcome"),
        "duration_ms": round(arrival - previous[0], 3), "samples": len(chains),
        "leaf": [{"symbol": name, "samples": count}
                 for name, count in leaves.most_common(20)],
        "inclusive": [{"symbol": name, "samples": count}
                      for name, count in inclusive.most_common(20)],
        "adjacent_calls": [{"caller": caller, "callee": callee, "samples": count}
                           for (caller, callee), count in adjacent.most_common(16)],
    })
    previous = (arrival, record)

failures = [index for index, window in enumerate(windows) if window["outcome"] == "fail"]
controls = sorted({j for i in failures for j in (i - 1, i + 1)
                   if 0 <= j < len(windows) and windows[j]["outcome"] == "pass"})
print(json.dumps({"failed_windows": [windows[i] for i in failures],
                  "adjacent_passing_controls": [windows[i] for i in controls]}, indent=2))
