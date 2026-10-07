#!/usr/bin/env python3
"""Verify committed fixture diagnostics against the explicitly pinned TypeScript 7 oracle.

The CLI exposes diagnostic code and start position, not diagnostic end spans. The committed
checker fixtures therefore keep the reviewed byte-range contract; this tool verifies the
oracle's code and start (line/column) and can only rewrite its separately marked evidence
after an explicit --update and an exact --confirm-version.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import tempfile
from pathlib import Path

FIXTURE_DIR = Path(__file__).resolve().parents[1] / "fixtures" / "differential"
EXPECTED_VERSION = "Version 7.0.2"
DIAGNOSTIC = re.compile(r"^(?P<file>.+?)\((?P<line>\d+),(?P<column>\d+)\): error TS(?P<code>\d+):")


def run(command: list[str], *, cwd: Path | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)


def relpath(path: str) -> str:
    if not path.startswith("/project/"):
        raise SystemExit(f"fixture path must live under /project: {path}")
    relative = Path(path.removeprefix("/project/"))
    if relative.is_absolute() or ".." in relative.parts:
        raise SystemExit(f"fixture path escapes /project: {path}")
    return str(relative)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tsc", type=Path, required=True, help="absolute path to the pinned tsc executable")
    parser.add_argument("--update", action="store_true", help="write verified oracle evidence into fixtures")
    parser.add_argument("--confirm-version", help="must exactly equal 'Version 7.0.2' when --update is used")
    args = parser.parse_args()
    version = run([str(args.tsc), "--version"])
    if version.returncode or version.stdout.strip() != EXPECTED_VERSION:
        raise SystemExit(f"expected pinned oracle {EXPECTED_VERSION!r}, got {version.stdout.strip()!r}")
    if args.update and args.confirm_version != EXPECTED_VERSION:
        raise SystemExit(f"--update requires --confirm-version {EXPECTED_VERSION!r}")

    fixtures = sorted(FIXTURE_DIR.glob("*.json"))
    if not fixtures:
        raise SystemExit(f"no differential fixtures found in {FIXTURE_DIR}")
    seen_ids: set[str] = set()
    failed = False
    for fixture_path in fixtures:
        data = json.loads(fixture_path.read_text())
        case_id = data.get("id")
        if not isinstance(case_id, str) or not case_id or case_id in seen_ids:
            raise SystemExit(f"{fixture_path.name}: missing or duplicate case id")
        seen_ids.add(case_id)
        options = data.get("options")
        files = data.get("files")
        roots = data.get("roots")
        if not isinstance(options, dict) or not isinstance(files, dict) or not isinstance(roots, list) or not roots:
            raise SystemExit(f"{case_id}: incomplete fixture inputs")
        expected = data.get("oracle")
        if not isinstance(expected, dict) or expected.get("version") != EXPECTED_VERSION or not isinstance(expected.get("diagnostics"), list):
            raise SystemExit(f"{case_id}: incomplete pinned oracle evidence")

        with tempfile.TemporaryDirectory(prefix="kiln-ts-diff-") as temp:
            project = Path(temp) / "project"
            project.mkdir()
            for virtual_path, contents in files.items():
                path = project / relpath(virtual_path)
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents)
            config_path = options.get("config_path")
            if not config_path:
                compiler_options = {
                    "target": "ESNext",
                    "module": "Preserve",
                    "strict": options.get("strict", True),
                    "noUncheckedIndexedAccess": options.get("no_unchecked_indexed_access", True),
                    "skipLibCheck": True,
                    "lib": options.get("libraries", ["es5"]),
                }
                (project / "tsconfig.json").write_text(
                    json.dumps({"compilerOptions": compiler_options, "files": [relpath(root) for root in roots]})
                )
                config_path = "/project/tsconfig.json"
            output = run(
                [str(args.tsc), "--pretty", "false", "--noEmit", "--project", str(project / relpath(config_path))],
                cwd=project,
            )
            actual = []
            unparsed_errors = []
            for line in output.stdout.splitlines():
                match = DIAGNOSTIC.match(line)
                if match:
                    try:
                        reported_path = Path(match.group("file"))
                        absolute_path = reported_path if reported_path.is_absolute() else project / reported_path
                        name = str(absolute_path.resolve().relative_to(project.resolve()))
                    except ValueError:
                        raise SystemExit(f"{case_id}: oracle reported a path outside the fixture: {line}")
                    actual.append({
                        "file": name,
                        "code": int(match.group("code")),
                        "line": int(match.group("line")),
                        "column": int(match.group("column")),
                    })
                elif "error TS" in line:
                    unparsed_errors.append(line)
            if unparsed_errors:
                raise SystemExit(f"{case_id}: unparsed TypeScript diagnostics:\n" + "\n".join(unparsed_errors))
            actual.sort(key=lambda row: (row["file"], row["line"], row["column"], row["code"]))
            expected_rows = expected["diagnostics"]
            if args.update:
                expected["diagnostics"] = actual
                fixture_path.write_text(json.dumps(data, indent=2) + "\n")
                print(f"updated {case_id}: {len(actual)} diagnostics ({EXPECTED_VERSION})")
            elif actual != expected_rows:
                failed = True
                print(f"FAIL {case_id}: expected {expected_rows!r}, got {actual!r}")
            else:
                print(f"OK   {case_id}: {len(actual)} diagnostics ({EXPECTED_VERSION})")
            if output.returncode not in (0, 1):
                failed = True
                print(f"{case_id}: tsc exited {output.returncode}:\n{output.stdout}")

    if len(seen_ids) != len(fixtures):
        raise SystemExit("one or more fixture cases lacked a unique name")
    fixture_digest = hashlib.sha256(
        b"".join(path.name.encode() + path.read_bytes() for path in fixtures)
    ).hexdigest()
    binary_digest = hashlib.sha256(args.tsc.read_bytes()).hexdigest()
    print(f"oracle={EXPECTED_VERSION} sha256={binary_digest}")
    print(f"fixture_inventory={len(fixtures)} sha256={fixture_digest}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
