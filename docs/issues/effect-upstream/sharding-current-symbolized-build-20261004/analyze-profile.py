#!/usr/bin/env python3
"""Reproduce module/hash and inclusive-stack attribution for a samply profile."""
import argparse
import bisect
import collections
import gzip
import json
from pathlib import Path

FNV_OFFSET_1 = 0xCBF29CE484222325
FNV_OFFSET_2 = 0x84222325CBF29CE4
FNV_PRIME = 0x100000001B3
PREFIX = b"kiln-logical-module-path\0"


def fnv(data: bytes, seed: int) -> int:
    for byte in data:
        seed = ((seed ^ byte) * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return seed


def logical_path_pair(path: str) -> str:
    encoded = path.encode("utf-8")
    payload = PREFIX + len(encoded).to_bytes(8, "little") + encoded
    return f"{fnv(payload, FNV_OFFSET_1):016x}{fnv(payload, FNV_OFFSET_2):016x}"


def symbol_for(address, starts, symbols, symbol_strings):
    if address is None or address < 0:
        return None
    i = bisect.bisect_right(starts, address) - 1
    if i < 0:
        return None
    symbol = symbols[i]
    if address >= symbol["rva"] + symbol["size"]:
        return None
    return symbol_strings[symbol["symbol"]]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("profile_gz", type=Path)
    parser.add_argument("symbols_json", type=Path)
    parser.add_argument("--markers-jsonl", type=Path,
                        help="newline-arrival records using monotonic_ns from the same host")
    parser.add_argument("--clock-receipt", type=Path,
                        help="receipt.json containing paired unix_ns/monotonic_ns observations")
    parser.add_argument("--source-root", type=Path,
                        help="root whose relative paths use kiln logical-module spelling")
    args = parser.parse_args()

    profile = json.load(gzip.open(args.profile_gz, "rt", encoding="utf-8"))
    symbols_doc = json.loads(args.symbols_json.read_text())
    image = next(item for item in symbols_doc["data"] if item["debug_name"] == "native.bin")
    symbols = sorted(image["symbol_table"], key=lambda item: item["rva"])
    starts = [item["rva"] for item in symbols]
    symbol_strings = symbols_doc["string_table"]
    thread = next(item for item in profile["threads"] if item["name"] == "native.bin")
    strings = thread["stringArray"]
    frames = thread["frameTable"]
    stacks = thread["stackTable"]
    funcs = thread["funcTable"]
    native_resource = next(
        index for index, library_index in enumerate(thread["resourceTable"]["lib"])
        if profile["libs"][library_index]["debugName"] == "native.bin"
    )

    def frame_name(frame_index):
        name = symbol_for(frames["address"][frame_index], starts, symbols, symbol_strings)
        if name:
            return name
        func = frames["func"][frame_index]
        return strings[funcs["name"][func]] if func is not None else "<unknown>"

    inclusive = collections.Counter()
    leaf = collections.Counter()
    parent = collections.Counter()
    native_samples = 0
    time_min = None
    time_max = None
    focused = {
        "kiln_c99_invoke_method_dispatch",
        "kiln_module_path_d26041571cf61f0bad70365407312980_fn_77",
        "kiln_module_path_9f9747a19882905c65626196c02048b1_fn_50",
    }
    timed_rows = []
    for sample_index, leaf_stack in enumerate(thread["samples"]["stack"]):
        if leaf_stack is None:
            continue
        chain = []
        stack_index = leaf_stack
        while stack_index is not None and stack_index >= 0:
            chain.append(frame_name(stacks["frame"][stack_index]))
            stack_index = stacks["prefix"][stack_index]
        if not chain:
            continue
        leaf_frame = stacks["frame"][leaf_stack]
        leaf_func = frames["func"][leaf_frame]
        if funcs["resource"][leaf_func] != native_resource:
            continue
        native_samples += 1
        sample_time = thread["samples"]["time"][sample_index]
        time_min = sample_time if time_min is None else min(time_min, sample_time)
        time_max = sample_time if time_max is None else max(time_max, sample_time)
        leaf[chain[0]] += 1
        inclusive.update(set(chain))
        timed_rows.append((sample_time, chain))
        for index, name in enumerate(chain):
            if name in focused:
                caller = chain[index + 1] if index + 1 < len(chain) else "<stack-root>"
                parent[(name, caller)] += 1

    path_map = {}
    if args.source_root:
        root = args.source_root.resolve()
        for path in root.rglob("*.ts"):
            rel = path.relative_to(root).as_posix()
            pair = logical_path_pair(rel)
            path_map.setdefault(pair, []).append(rel)

    focus_report = []
    for name in sorted(focused):
        pair = name[len("kiln_module_path_"):].split("_fn_")[0] if name.startswith("kiln_module_path_") else None
        callers = [{"symbol": caller, "samples": count}
                   for (target, caller), count in parent.most_common() if target == name][:10]
        focus_report.append({
            "symbol": name,
            "inclusive_samples": inclusive[name],
            "inclusive_percent_of_native_samples": round(inclusive[name] * 100 / native_samples, 3),
            "leaf_samples": leaf[name],
            "leaf_percent_of_native_samples": round(leaf[name] * 100 / native_samples, 3),
            "logical_module_hash_pair": pair,
            "matching_source_paths": path_map.get(pair, []) if pair else [],
            "outward_callers": callers,
        })

    report = {
        "profile_image": {"debug_name": image["debug_name"], "code_id": image["code_id"]},
        "profile_native_samples": native_samples,
        "sample_time_ms_range": [time_min, time_max],
        "top_inclusive_symbols": [
            {"symbol": name, "samples": count,
             "percent_of_native_samples": round(count * 100 / native_samples, 3)}
            for name, count in inclusive.most_common(35)
        ],
        "top_leaf_symbols": [
            {"symbol": name, "samples": count,
             "percent_of_native_samples": round(count * 100 / native_samples, 3)}
            for name, count in leaf.most_common(35)
        ],
        "focused_symbols": focus_report,
    }
    if args.markers_jsonl:
        events = []
        for line in args.markers_jsonl.read_text().splitlines():
            marker = json.loads(line)
            record = marker["record"]
            if record.get("kind") in ("inventory", "case"):
                events.append((marker["monotonic_ns"] / 1_000_000, record))

        windows = []
        previous_time = None
        previous_record = None
        interesting = set(focus for focus in focused if focus.startswith("kiln_module_path_"))
        interesting.update({"kiln_c99_invoke_method_dispatch", "kiln_c99_run_timer"})
        for arrival_time, record in events:
            if record["kind"] == "inventory":
                previous_time, previous_record = arrival_time, record
                continue
            if previous_time is None:
                previous_time, previous_record = arrival_time, record
                continue
            rows = [chain for sample_time, chain in timed_rows
                    if previous_time <= sample_time <= arrival_time]
            counts = collections.Counter()
            leaf_counts = collections.Counter()
            for chain in rows:
                leaf_counts[chain[0]] += 1
                counts.update(set(chain))
            windows.append({
                "case_id": record.get("id"),
                "outcome": record.get("outcome"),
                "start_marker": previous_record.get("kind"),
                "duration_ms_between_arrivals": round(arrival_time - previous_time, 3),
                "native_samples": len(rows),
                "symbols": {
                    name: {"inclusive_samples": counts[name],
                          "leaf_samples": leaf_counts[name],
                          "inclusive_percent_of_window": round(counts[name] * 100 / len(rows), 2)
                              if rows else None}
                    for name in sorted(interesting)
                },
            })
            previous_time, previous_record = arrival_time, record

        failure_controls = []
        for index, row in enumerate(windows):
            if row["outcome"] != "fail":
                continue
            neighbors = []
            for candidate_index in (index - 1, index + 1):
                if 0 <= candidate_index < len(windows):
                    candidate = windows[candidate_index]
                    if candidate["outcome"] == "pass":
                        neighbors.append(candidate)
            failure_controls.append({"failure": row, "adjacent_passing_controls": neighbors})
        report["marker_alignment"] = {
            "clock": "profile sample time and marker monotonic_ns/1e6 are both host monotonic milliseconds",
            "interpretation": "arrival-to-arrival intervals include harness hooks, cleanup and setup; not callback-only execution",
            "case_windows": windows,
            "failures_with_adjacent_controls": failure_controls,
        }
        if args.clock_receipt:
            receipt = json.loads(args.clock_receipt.read_text())
            start = receipt["clock_pairs_at_start"]
            end = receipt["clock_pairs_at_end"]
            start_offset = start["unix_ns"] - start["monotonic_ns"]
            end_offset = end["unix_ns"] - end["monotonic_ns"]
            marker_first = events[0][0]
            marker_last = events[-1][0]
            report["clock_alignment"] = {
                "unix_minus_monotonic_start_ns": start_offset,
                "unix_minus_monotonic_end_ns": end_offset,
                "offset_drift_ns": end_offset - start_offset,
                "profile_thread_register_ms": thread["registerTime"],
                "first_event_marker_ms": marker_first,
                "first_marker_minus_thread_register_ms": round(marker_first - thread["registerTime"], 3),
                "profile_first_sample_ms": min(thread["samples"]["time"]),
                "profile_last_sample_ms": max(thread["samples"]["time"]),
                "last_event_marker_ms": marker_last,
                "last_profile_sample_minus_last_marker_ms": round(max(thread["samples"]["time"]) - marker_last, 3),
            }
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
