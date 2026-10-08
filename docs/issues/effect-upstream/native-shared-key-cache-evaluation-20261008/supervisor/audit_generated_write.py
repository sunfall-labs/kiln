#!/usr/bin/env python3
"""Extract the actual benchmark module and prove its hot writer calls the dynamic setter."""
import argparse
import hashlib
import json
from pathlib import Path
import re

BENCH = Path(__file__).resolve().parent
EXPECTED_ROWS = {
    "shared-canonical-dynamic-write": "33558527",
    "realm-local-signature-collision-write": "33558527",
    "realm-local-distinct-signature-write": "33558527",
    "warmed-own-slot-read-regression-control": "67100672",
}
EXPECTED_SOURCE_SHA256 = "eaeead3d97e32e0508321fe18e4713223fbe37a811bac15fa97a7b7114579d54"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def extract(cache_root: Path) -> dict:
    if sha((BENCH / "project-boundary/workload.ts").read_bytes()) != EXPECTED_SOURCE_SHA256:
        raise RuntimeError("frozen workload source digest mismatch")
    generated = cache_root / "generated-c"
    if not generated.is_dir():
        raise RuntimeError(f"generated-C cache missing: {generated}")
    hits = []
    for path in generated.glob("*/*"):
        if path.name.endswith(".id") or not path.is_file():
            continue
        record = path.read_bytes()
        if len(record) < 82 or record[:8] != b"KILNART\0":
            continue
        version = int.from_bytes(record[8:10], "little")
        if version != 3:
            continue
        payload = record[82:]
        if len(payload) != int.from_bytes(record[74:82], "little"):
            continue
        if sha(payload) != record[42:74].hex():
            raise RuntimeError(f"generated-C payload digest mismatch: {path}")
        try:
            fields = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError):
            continue
        if not isinstance(fields, list) or len(fields) != 9:
            continue
        module_id, module_symbol, c_text = fields[:3]
        if not isinstance(c_text, str) or '"workload.ts"' not in c_text:
            continue
        source_markers = {
            "writer_definition": c_text.count("function repeatedComputedWrite("),
            "reader_definition": c_text.count("function repeatedOwnSlotRead("),
            "writer_row_closures": c_text.count("() => repeatedComputedWrite("),
            "reader_row_closures": c_text.count("() => repeatedOwnSlotRead("),
        }
        expected_source_markers = {
            "writer_definition": 1, "reader_definition": 1,
            "writer_row_closures": 3, "reader_row_closures": 1,
        }
        if source_markers != expected_source_markers:
            raise RuntimeError(f"workload source/C function-row inventory mismatch: {path}: {source_markers}")
        names = {}
        marker = re.compile(r"/\* function (\d+): ([^*]+) \*/\s*/\* frontend function \1:")
        for match in marker.finditer(c_text):
            names[int(match.group(1))] = match.group(2).strip()
        writers = []
        for index, name in names.items():
            if name != "repeatedComputedWrite":
                continue
            symbol = f"ku_fn_{index}(KU_PARAMS) {{"
            start = c_text.find(symbol)
            if start < 0:
                raise RuntimeError(f"missing generated body for {symbol}")
            end = c_text.find("\n}\n", start)
            if end < 0:
                raise RuntimeError(f"unterminated generated body for {symbol}")
            body = c_text[start : end + 2]
            writers.append({
                "proto_index": index,
                "symbol": symbol[:-2],
                "body_sha256": sha(body.encode()),
                "body_bytes": len(body.encode()),
                "val_write_element_calls": body.count("kiln_c99_val_write_element("),
                "set_computed_mode_calls": body.count("kiln_c99_set_computed_mode("),
            })
        if writers:
            hits.append({
                "record_path": str(path.resolve()),
                "record_sha256": sha(record),
                "record_bytes": len(record),
                "payload_sha256": sha(payload),
                "module_id": module_id,
                "module_symbol": module_symbol,
                "module_c_sha256": sha(c_text.encode()),
                "module_c_bytes": len(c_text.encode()),
                "source_function_row_markers": source_markers,
                "function_names": names,
                "timed_write_functions": writers,
            })
    if len(hits) != 1:
        raise RuntimeError(f"expected one workload generated-C record; found {len(hits)}")
    row = hits[0]
    functions = row["timed_write_functions"]
    if len(functions) != 1 or functions[0]["val_write_element_calls"] < 1:
        raise RuntimeError(f"hot computed writer did not emit val_write_element: {functions}")
    return {
        "status": "verified",
        "scope": "emitted module caller proof; runtime intern_key path is tied to pinned C source receipt",
        "row_inventory": EXPECTED_ROWS,
        "workload_module": row,
        "regression_only_read_row": "warmed-own-slot-read-regression-control",
        "note": "This proves the generated hot function reaches the runtime computed-write entry; it does not itself prove a particular intern-cache hit at runtime.",
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache-root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    workload = BENCH / "project-boundary/workload.ts"
    if sha(workload.read_bytes()) != EXPECTED_SOURCE_SHA256:
        raise RuntimeError("frozen workload source digest mismatch")
    result = extract(args.cache_root)
    result["cache_root"] = str(args.cache_root.resolve())
    result["workload_source_sha256"] = EXPECTED_SOURCE_SHA256
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "payload_sha256": result["workload_module"]["payload_sha256"],
                      "writer": result["workload_module"]["timed_write_functions"]}, indent=2))


if __name__ == "__main__":
    main()
