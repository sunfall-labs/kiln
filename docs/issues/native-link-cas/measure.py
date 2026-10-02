"""Compare cold native links with identical programs, provider and worker limits.

Usage: python3 measure.py <kiln-binary> <unique-run-name>
Each run gets a new artifact cache, while every run uses the same project paths.
The wait4 I/O counters cover each kiln process and any compiler/linker children
it waits for. They compare total cold-link I/O; they do not isolate staging.
"""

import collections
import hashlib
import json
import os
from pathlib import Path
import sys
import time


ROOT = Path(__file__).resolve().parent
PROJECT = ROOT / "project"
CASES = 12
PARALLEL = 2
binary = Path(sys.argv[1]).resolve()
label = sys.argv[2]
run = ROOT / label
run.mkdir(exist_ok=False)
PROJECT.mkdir(exist_ok=True)

package = PROJECT / "package.json"
expected_package = '{"type":"module"}\n'
if package.exists():
    assert package.read_text() == expected_package
else:
    package.write_text(expected_package)

for index in range(CASES):
    for name, content in (
        (f"main{index}.js", f'import {{ answer }} from "./dep{index}.js"; console.log(answer);\n'),
        (f"dep{index}.js", f"export const answer = {index};\n"),
    ):
        path = PROJECT / name
        if path.exists():
            assert path.read_text() == content
        else:
            path.write_text(content)

wrapper = ROOT / "cc"
wrapper.write_text(
    '''#!/usr/bin/env bash
for arg in "$@"; do
    if [[ $arg == -c ]]; then
        printf 'compile\\n' >> "$KILN_LINK_CAS_COMPILES"
        break
    fi
done
exec ~/.cache/kiln/clang-18/usr/bin/clang-18 "$@"
'''
)
wrapper.chmod(0o755)

environment = dict(
    os.environ,
    CC=str(wrapper),
    KILN_JOBS="1",
    RAYON_NUM_THREADS="1",
    KILN_SHARED_CACHE=str(run / "cache"),
    KILN_LINK_CAS_COMPILES=str(run / "compiles.log"),
    LD_LIBRARY_PATH=(
        "~/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:"
        "~/.cache/kiln/clang-18/usr/lib/llvm-18/lib"
    ),
)


def start_case(index):
    pid = os.fork()
    if pid == 0:
        try:
            os.chdir(PROJECT)
            with (run / f"{index}.stdout").open("wb") as stdout:
                with (run / f"{index}.stderr").open("wb") as stderr:
                    os.dup2(stdout.fileno(), 1)
                    os.dup2(stderr.fileno(), 2)
                    os.execve(
                        str(binary),
                        [str(binary), "--one-shot", "run", "--native", f"main{index}.js"],
                        environment,
                    )
        except BaseException as error:
            os.write(2, f"exec failed: {error}\n".encode())
        os._exit(127)
    return pid


started = time.monotonic()
running = {}
next_case = 0
results = {}
while next_case < CASES and len(running) < PARALLEL:
    running[start_case(next_case)] = next_case
    next_case += 1

while running:
    pid, status, usage = os.wait4(-1, 0)
    index = running.pop(pid)
    result = {
        "index": index,
        "status": os.waitstatus_to_exitcode(status),
        "stdout": (run / f"{index}.stdout").read_text(),
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "output_blocks": usage.ru_oublock,
        "input_blocks": usage.ru_inblock,
    }
    results[index] = result
    if next_case < CASES:
        running[start_case(next_case)] = next_case
        next_case += 1

elapsed = time.monotonic() - started
ordered = [results[index] for index in range(CASES)]
counts = collections.Counter(line.strip() for line in (run / "compiles.log").read_text().splitlines())
document = {
    "label": label,
    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    "wall_seconds": elapsed,
    "cli_user_seconds": sum(item["user_seconds"] for item in ordered),
    "cli_system_seconds": sum(item["system_seconds"] for item in ordered),
    "cli_output_blocks": sum(item["output_blocks"] for item in ordered),
    "cli_input_blocks": sum(item["input_blocks"] for item in ordered),
    "c_compiles": counts["compile"],
    "cases": ordered,
    "workload": f"{CASES} unique two-module programs, {PARALLEL} concurrent CLI processes",
    "artifact_cache": "fresh per run, shared only among these CLI processes",
    "provider_archive": "already built before either version",
}
(run / "result.json").write_text(json.dumps(document, indent=2) + "\n")
print(json.dumps({key: value for key, value in document.items() if key != "cases"}))
assert all(
    item["status"] == 0 and item["stdout"] == f'{item["index"]}\n'
    for item in ordered
), ordered
