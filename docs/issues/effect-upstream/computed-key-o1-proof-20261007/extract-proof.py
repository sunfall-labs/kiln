#!/usr/bin/env python3
"""Validate and extract the bounded computed-key ranking provenance evidence."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path


ARTIFACT = Path(
    "~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/"
    "frozen-clock-native/workspace/.kiln/cache/generated-c/f6/"
    "f6130aa23635332d913f66e077c4bf97deaa2d3639bf1f7912c090cb93129cd1"
)
SOURCE = Path(
    "~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/"
    "frozen-clock-native/workspace/source/packages/effect/src/internal/effect.ts"
)
EXPECTED_RECORD_SHA256 = "b3c54f19e6f83bce9d3ff3337e2898731dc995e365e6054ba109c7576e02d473"
EXPECTED_PAYLOAD_SHA256 = "40abce548e10301f3b35c1160673b475767e63009749b712f60d0fbbe0ee00fc"
EXPECTED_SOURCE_SHA256 = "68c43e18e167d17b39f224d3793f11c7732cd857b7908ecd3ff1d1edf0affac2"
EXPECTED_SOURCE_COMMIT = "67ba4e46a11ccda0b6761578bfd22c04ae00167d"
MODULE_SYMBOL = "kiln_module_path_9f9747a19882905c65626196c02048b1"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def runtime_import_bindings(source: str) -> list[tuple[str, str]]:
    """Return runtime bindings in authored import order; erase whole type-only imports."""
    pattern = re.compile(
        r"^import\s+(?!type\b)(?P<clause>.*?)\s+from\s+['\"](?P<module>[^'\"]+)['\"]",
        re.MULTILINE | re.DOTALL,
    )
    bindings: list[tuple[str, str]] = []
    for match in pattern.finditer(source):
        clause = " ".join(match.group("clause").split())
        module = match.group("module")
        if clause.startswith("{"):
            names = [
                part.strip().split(" as ", 1)[-1].strip()
                for part in clause[1 : clause.rfind("}")].split(",")
                if part.strip() and not part.strip().startswith("type ")
            ]
        elif clause.startswith("*"):
            names = [clause.split(" as ", 1)[1].strip()]
        else:
            default = clause.split(",", 1)[0].strip()
            names = [default] if default else []
            if "," in clause:
                rest = clause.split(",", 1)[1].strip()
                if rest.startswith("{"):
                    names.extend(
                        part.strip().split(" as ", 1)[-1].strip()
                        for part in rest[1 : rest.rfind("}")].split(",")
                        if part.strip() and not part.strip().startswith("type ")
                    )
                elif rest.startswith("*"):
                    names.append(rest.split(" as ", 1)[1].strip())
        bindings.extend((name, module) for name in names)
    return bindings


def function_record(c_text: str, index: int) -> tuple[int, int, str, str]:
    marker = re.compile(
        rf"/\* function {index}(?:: [^*]+)? \*/\s*"
        rf"/\* frontend function {index}: bytes (\d+)\.\.(\d+); ([^*]+) \*/"
    )
    match = marker.search(c_text)
    if not match:
        raise AssertionError(f"missing frontend function metadata for fn{index}")
    start, end = int(match.group(1)), int(match.group(2))
    symbol = f"{MODULE_SYMBOL}_fn_{index}(KU_PARAMS) {{"
    body_start = c_text.find(symbol, match.end())
    if body_start < 0:
        raise AssertionError(f"missing emitted body for fn{index}")
    body_end = c_text.find("\n}", body_start)
    if body_end < 0:
        raise AssertionError(f"unterminated emitted body for fn{index}")
    return start, end, match.group(3).strip(), c_text[body_start : body_end + 2]


def closure_vector(c_text: str, index: int) -> str:
    pattern = re.compile(
        rf"\{{ kiln_val c\[(\d+)\] = \{{ ([^}}]+) \}}; "
        rf"KU_CLOSURE\([^\n]*, {index}, [^\n]*\);? \}}"
    )
    matches = list(pattern.finditer(c_text))
    if len(matches) != 1:
        raise AssertionError(f"expected one emitted closure site for fn{index}, got {len(matches)}")
    match = matches[0]
    values = [value.strip() for value in match.group(2).split(",")]
    return f"capture_count={match.group(1)} vector={values}"


def main() -> None:
    artifact = Path(sys.argv[1]) if len(sys.argv) > 1 else ARTIFACT
    source_path = Path(sys.argv[2]) if len(sys.argv) > 2 else SOURCE
    record = artifact.read_bytes()
    if record[:8] != b"KILNART\0" or int.from_bytes(record[8:10], "little") != 3:
        raise AssertionError("unexpected artifact record magic/version")
    if sha256(record) != EXPECTED_RECORD_SHA256:
        raise AssertionError(f"unexpected full artifact digest: {sha256(record)}")
    payload = record[82:]
    if sha256(payload) != EXPECTED_PAYLOAD_SHA256 or record[42:74].hex() != EXPECTED_PAYLOAD_SHA256:
        raise AssertionError("artifact payload digest mismatch")
    if int.from_bytes(record[74:82], "little") != len(payload):
        raise AssertionError("artifact payload length mismatch")
    payload_fields = json.loads(payload)
    module_id, symbol, c_text = payload_fields[:3]
    if len(payload_fields) != 9:
        raise AssertionError(f"unexpected generated-C payload fields: {len(payload_fields)}")
    if symbol != MODULE_SYMBOL:
        raise AssertionError(f"unexpected module symbol {symbol}")

    source_bytes = source_path.read_bytes()
    source_sha = sha256(source_bytes)
    if source_sha != EXPECTED_SOURCE_SHA256:
        raise AssertionError(f"unexpected pinned source digest: {source_sha}")
    source_text = source_bytes.decode("utf-8")
    source_root = source_path.parents[4]
    source_commit = subprocess.check_output(
        ["git", "-C", str(source_root), "rev-parse", "HEAD"], text=True
    ).strip()
    if source_commit != EXPECTED_SOURCE_COMMIT:
        raise AssertionError(f"unexpected pinned source commit: {source_commit}")
    imports = runtime_import_bindings(source_text)
    if len(imports) <= 40 or imports[40] != ("evaluate", "./core.ts"):
        raise AssertionError(f"runtime import slot40 mismatch: {imports[40:41]}")

    print(f"artifact={artifact}")
    print(f"record_sha256={sha256(record)} payload_sha256={sha256(payload)} module_id={module_id}")
    print(
        f"module_symbol={symbol} source={source_path} source_sha256={source_sha} "
        f"source_commit={source_commit}"
    )
    print(f"runtime_import[40]={imports[40][0]} from {imports[40][1]}")

    for index in (50, 120, 123):
        start, end, flags, body = function_record(c_text, index)
        print(f"fn{index}_metadata=bytes {start}..{end}; {flags}")
        if index in (50, 123):
            print(f"fn{index}_closure={closure_vector(c_text, index)}")
        if index == 50:
            assert 'upvalues[4], thrown, 112, "LoadUpvalue"' in body
            assert '(size_t)113, "ToPropertyKey"' in body
            assert '(size_t)117, "CallComputed"' in body
            print("fn50_call=PC112 LoadUpvalue(upvalues[4]) -> PC113 ToPropertyKey -> PC117 CallComputed")
        if index == 123:
            assert 'upvalues[1], thrown, 11, "LoadUpvalue"' in body
            assert '(size_t)12, "ToPropertyKey"' in body
            assert 'KU_CLOSURE(v16, 120,' in body
            assert '(size_t)14, "DefineNamedProperty"' in body
            print("fn123_definition=PC11 LoadUpvalue(upvalues[1]) -> PC12 ToPropertyKey -> closure fn120 -> PC14 DefineNamedProperty")

    init = "{ kiln_val imported = kiln_val_from_boundary(imports[40]);"
    init_start = c_text.find(init)
    if init_start < 0:
        raise AssertionError("missing import[40] -> locals[40] initialization")
    init_end = c_text.find("\n", init_start)
    cell_start = c_text.find("locals[40] & KILN_TAG_MASK", init_end)
    cell_end = c_text.find("\n", cell_start)
    print("module_import_cell_init=" + c_text[init_start:cell_end].strip())

    # Frontend byte spans are compiler coordinates. TypeScript erasure/lowering can
    # change coordinates, so do not interpret them as offsets into authored source.
    needle = "(current as any)[evaluate](this)"
    source_pos = source_text.find(needle)
    if source_pos < 0:
        raise AssertionError("expected computed evaluate call absent from pinned source")
    print(f"raw_frontend_spans=fn50:{function_record(c_text, 50)[0]}..{function_record(c_text, 50)[1]}; "
          f"fn120:{function_record(c_text, 120)[0]}..{function_record(c_text, 120)[1]}; "
          f"fn123:{function_record(c_text, 123)[0]}..{function_record(c_text, 123)[1]}")
    print(f"source_contains_computed_evaluate_call=true byte_offset={source_pos}")


if __name__ == "__main__":
    main()
