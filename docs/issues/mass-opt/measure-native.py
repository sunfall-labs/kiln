"""Compare copied native runners on the same corpus and populated artifact cache."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time

root = Path(__file__).resolve().parents[2]
out = root / 'docs/issues/mass-opt'
env = dict(os.environ, KILN_JOBS='8',
    CC='~/.cache/kiln/clang-18/usr/bin/clang-18',
    LD_LIBRARY_PATH='~/.cache/kiln/clang-18/usr/lib/x86_64-linux-gnu:~/.cache/kiln/clang-18/usr/lib/llvm-18/lib')
for label, name in [('baseline-warm', 'baseline-kiln'), ('candidate-warm', 'hash-bootstrap-kiln')]:
    binary = out / name
    env['KILN_BIN'] = str(binary)
    started = time.monotonic()
    with (out / f'native-{label}.log').open('w') as log:
        result = subprocess.run([str(root / 'scripts/native.sh')], cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
    text = (out / f'native-{label}.log').read_text()
    summary = [line for line in text.splitlines() if re.search(r'\d+ matched.*\d+ refused.*\d+ diverged', line)]
    record = dict(seconds=time.monotonic()-started, status=result.returncode,
        binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), jobs=8,
        summary=summary, cache_condition='Previously ran the whole corpus using this binary in this worktree')
    (out / f'native-{label}.json').write_text(json.dumps(record, indent=2)+'\n')
    print(label, record, flush=True)
    if result.returncode or not summary:
        raise SystemExit('Native comparison failed or produced no recognized summary')
