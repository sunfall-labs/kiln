"""Observe unchanged native stdout externally; do not modify tests or deadlines."""
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time

ROOT = Path('~/Work/kiln-effect-next-milestone')
WORKSPACE = Path('~/.cache/kiln/effect-final-322-20261004')
BASE = ROOT / '.scratch/sharding-current-symbolized-prebuild'
OUT = BASE / 'result-marker-profile'
BINARY = BASE / 'native.bin'
EXPECTED_SHA = '1daaeb90e8c07f7846d913329f00d4e36fe12cd1f2327d3430feab49afbdbe8f'
OUT.mkdir(exist_ok=True)
if (OUT / 'receipt.json').exists():
    raise SystemExit('Refusing to overwrite an existing diagnostic run')
sha = hashlib.file_digest(BINARY.open('rb'), 'sha256').hexdigest()
if sha != EXPECTED_SHA:
    raise SystemExit('Native binary digest differs from the profiled candidate')
env = dict(os.environ, TZ='UTC', KILN_JOBS='1', RAYON_NUM_THREADS='1')
for name in ('KILN_GC_STRESS', 'KILN_NATIVE_GC_STRESS'):
    env.pop(name, None)
command = ['samply', 'record', '--save-only', '--unstable-presymbolicate', '--rate', '500',
           '--output', str(OUT / 'profile.json.gz'), sys.executable,
           str(ROOT / 'scripts/effect-upstream.py'), '--limited-child', '8192', str(BINARY)]
receipt = {'status': 'running', 'binary_sha256': sha, 'command': command, 'cwd': str(WORKSPACE),
           'case_deadlines': 'unchanged upstream deadlines', 'outer_timeout_s': 2400,
           'stress_environment': 'both variables unset',
           'observer': 'external timestamps at arrival of newline-terminated stdout records',
           'window_limits': 'Intervals between result markers include hooks, cleanup, setup and possibly background tasks; not exact callback-only timings.',
           'stdout_flush_evidence': 'crates/native/kiln-native-runtime/src/lib.rs native console host path flushes sink after newline (line 1292 at validated source).',
           'clock_pairs_at_start': {'unix_ns': time.time_ns(), 'monotonic_ns': time.monotonic_ns()}}
(OUT / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
started = time.monotonic()
child = subprocess.Popen(command, cwd=WORKSPACE, env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, start_new_session=True)
selector = selectors.DefaultSelector()
selector.register(child.stdout, selectors.EVENT_READ)
pending = b''
markers = []
timed_out = False
with (OUT / 'native-and-profiler.log').open('wb') as raw, (OUT / 'marker-arrivals.jsonl').open('w') as arrivals:
    while selector.get_map():
        if time.monotonic() - started >= 2400 and not timed_out:
            timed_out = True
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for key, _ in selector.select(0.25):
            data = os.read(key.fileobj.fileno(), 65536)
            if not data:
                selector.unregister(key.fileobj)
                continue
            raw.write(data)
            raw.flush()
            pending += data
            while b'\n' in pending:
                line, pending = pending.split(b'\n', 1)
                if not line.startswith(b'KILN_EFFECT '):
                    continue
                observed = {'unix_ns': time.time_ns(), 'monotonic_ns': time.monotonic_ns(),
                            'record': json.loads(line[len(b'KILN_EFFECT '):])}
                markers.append(observed)
                arrivals.write(json.dumps(observed, separators=(',', ':')) + '\n')
                arrivals.flush()
    code = child.wait()
receipt.update(status='finished', profiler_exit_code=code, outer_timed_out=timed_out,
               wall_s=time.monotonic() - started,
               clock_pairs_at_end={'unix_ns': time.time_ns(), 'monotonic_ns': time.monotonic_ns()},
               marker_count=len(markers),
               summary=[row['record'] for row in markers if row['record']['kind'] == 'summary'],
               native_exit_limit='The profiler exit is not asserted to equal the profiled native process exit.')
(OUT / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt['summary']), 'profiler_exit', code, 'outer_timeout', timed_out, flush=True)
