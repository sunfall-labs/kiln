#!/usr/bin/env python3
"""Compare two prebuilt native binaries against the fixed-salt Node checksum."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--candidate', required=True, type=Path)
parser.add_argument('--control', required=True, type=Path)
parser.add_argument('--candidate-source', required=True)
parser.add_argument('--control-source', required=True)
parser.add_argument('--out', required=True, type=Path)
args = parser.parse_args()
probe = Path(__file__).with_name('bcrypt-typed-read.mjs').resolve()
reference = subprocess.run(['node', str(probe)], capture_output=True, check=True).stdout
report = {
    'schema': 1,
    'timing_kind': 'native process phase wall time including launch and module setup; not workload throughput',
    'probe_sha256': hashlib.sha256(probe.read_bytes()).hexdigest(),
    'node_checksum_sha256': hashlib.sha256(reference).hexdigest(),
    'candidate_source': args.candidate_source,
    'control_source': args.control_source,
    'binaries': {},
    'samples': [],
}
for name in ['candidate', 'control']:
    binary = getattr(args, name).resolve()
    report['binaries'][name] = {'path': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
    # A warmup catches checksum/exit differences before they become timing rows.
    warm = subprocess.run([str(binary)], capture_output=True, timeout=120)
    if warm.returncode != 0 or warm.stdout != reference:
        raise SystemExit(f'{name} warmup diverged: exit={warm.returncode}, stderr={warm.stderr.decode(errors="replace")}')
for pair in range(5):
    order = ['candidate', 'control'] if pair % 2 == 0 else ['control', 'candidate']
    for name in order:
        start = time.perf_counter()
        run = subprocess.run([report['binaries'][name]['path']], capture_output=True, timeout=120)
        elapsed = time.perf_counter() - start
        if run.returncode != 0 or run.stdout != reference:
            raise SystemExit(f'{name} pair {pair} diverged: exit={run.returncode}, stderr={run.stderr.decode(errors="replace")}')
        report['samples'].append({'pair': pair, 'name': name, 'seconds': elapsed})
        print(f'{pair} {name}: {elapsed:.6f}s', flush=True)
report['medians_seconds'] = {
    name: statistics.median(row['seconds'] for row in report['samples'] if row['name'] == name)
    for name in ['candidate', 'control']
}
report['control_over_candidate'] = report['medians_seconds']['control'] / report['medians_seconds']['candidate']
args.out.parent.mkdir(parents=True, exist_ok=True)
args.out.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'medians_seconds': report['medians_seconds'], 'control_over_candidate': report['control_over_candidate']}))
