#!/usr/bin/env python3
"""Reproduce the generated-function leaf/inclusive tables from the retained profile."""
import argparse
import bisect
import collections
import gzip
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("profile", type=Path)
    parser.add_argument("symbols", type=Path)
    parser.add_argument("--tiers", type=Path, required=True)
    args = parser.parse_args()

    profile = json.load(gzip.open(args.profile, "rt", encoding="utf-8"))
    symbols_doc = json.loads(args.symbols.read_text())
    images = [image for image in symbols_doc["data"] if image["debug_name"] == "Sharding.native"]
    if len(images) != 1:
        raise SystemExit(f"expected one Sharding.native symbol image; found {len(images)}")
    image = images[0]
    profile_image = next(library for library in profile["libs"]
                         if library["debugName"] == "Sharding.native")
    if image["code_id"] != profile_image["codeId"]:
        raise SystemExit("symbol sidecar code ID does not match the profiled executable")

    symbol_rows = sorted(image["symbol_table"], key=lambda row: row["rva"])
    starts = [row["rva"] for row in symbol_rows]
    symbol_strings = symbols_doc["string_table"]

    def resolve(address):
        if address is None:
            return None
        index = bisect.bisect_right(starts, address) - 1
        if index < 0:
            return None
        row = symbol_rows[index]
        if address >= row["rva"] + row["size"]:
            return None
        return symbol_strings[row["symbol"]]

    thread = next(thread for thread in profile["threads"] if thread["name"] == "Sharding.native")
    native_library = next(i for i, library in enumerate(profile["libs"])
                          if library["debugName"] == "Sharding.native")
    native_resource = next(i for i, resource in enumerate(thread["resourceTable"]["lib"])
                           if resource == native_library)
    frames = thread["frameTable"]
    stacks = thread["stackTable"]
    leaf = collections.Counter()
    inclusive = collections.Counter()
    native_samples = 0

    for stack in thread["samples"]["stack"]:
        if stack is None:
            continue
        frame = stacks["frame"][stack]
        function = frames["func"][frame]
        if function is None or thread["funcTable"]["resource"][function] != native_resource:
            continue
        native_samples += 1
        name = resolve(frames["address"][frame])
        if name:
            leaf[name] += 1
        seen = set()
        cursor = stack
        while cursor is not None and cursor >= 0:
            frame = stacks["frame"][cursor]
            name = resolve(frames["address"][frame])
            if name and "_fn_" in name:
                seen.add(name)
            cursor = stacks["prefix"][cursor]
        inclusive.update(seen)

    metadata = json.loads(args.tiers.read_text())
    total_samples = len(thread["samples"]["stack"])
    tier_rows = {row["symbol"]: row for module in metadata["modules"]
                 for row in metadata["top_generated_c_leaf_functions"]
                 if row["symbol"].startswith("kiln_module_path_" + module["module_hash_pair"])}
    leaf_functions = []
    for name, count in leaf.most_common():
        if "_fn_" not in name:
            continue
        row = tier_rows.get(name)
        leaf_functions.append({
            "symbol": name,
            "samples": count,
            "percent_of_native_samples": round(100 * count / native_samples, 3),
            "tier": row["tier"] if row else None,
            "ir_instructions": row["ir_instructions"] if row else None,
            "publishes_frame": row["publishes_frame"] if row else None,
        })
    inclusive_functions = [
        {"symbol": name, "samples": count,
         "percent_of_native_samples": round(100 * count / native_samples, 3),
         "leaf_samples": leaf[name]}
        for name, count in inclusive.most_common(30)
    ]
    print(json.dumps({
        "profile_native_samples": native_samples,
        "profile_total_samples": total_samples,
        "top_generated_c_leaf_functions": leaf_functions[:30],
        "top_generated_c_inclusive_functions": inclusive_functions,
        "note": "Inclusive sample counts overlap along call stacks; tier metadata is limited to the reviewed top-15 evidence rows."
    }, indent=2))


if __name__ == "__main__":
    main()
