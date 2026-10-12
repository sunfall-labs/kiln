#!/usr/bin/env python3
"""Snapshot conformance baselines from an immutable source-repository revision."""
import argparse
import csv
import io
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--repo', required=True, help='kiln source checkout')
parser.add_argument('--ref', default='v0.1.0', help='release tag or commit to publish')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
commit = subprocess.check_output(['git', 'rev-parse', f'{args.ref}^{{commit}}'], cwd=args.repo, text=True).strip()
def load(path):
    return json.loads(subprocess.check_output(['git', 'show', f'{commit}:{path}'], cwd=args.repo))
baseline = load('conformance/test262-baseline.json')
ts = load('conformance/ts-conformance-baseline.json')
diff = load('conformance/tsc-differential-baseline.json')
if sum(ts['levels'].values()) != ts['tests'] or len(diff['cases']) != diff['matching']:
    raise SystemExit('Conformance totals do not match their recorded inventories')
groups = {}
for name, counts in baseline['chunks'].items():
    group = groups.setdefault(name.split('/')[0], dict(passed=0, failed=0, chunks=0))
    for field in ('passed', 'failed'):
        group[field] += counts[field]
    group['chunks'] += 1
record = dict(release=args.ref, source_commit=commit, test262=baseline, typescript=ts, differential=diff, groups=groups)
(root / 'data/conformance.json').write_text(json.dumps(record, indent=2) + '\n')
print(f'Imported pinned baselines from {commit}')

result_path = 'conformance/npm-compat/results/2026-10-11.tsv'
raw = subprocess.check_output(['git', 'show', f'{commit}:{result_path}'], cwd=args.repo, text=True)
packages = list(csv.DictReader(io.StringIO(raw), delimiter='\t'))
manifest = load('conformance/npm-compat/package.json')['dependencies']
if len(packages) != len(manifest) or len({p['package'] for p in packages}) != len(packages):
    raise SystemExit('npm results do not match the complete pinned inventory')
if {p['package']: p['version'] for p in packages} != manifest:
    raise SystemExit('npm result versions differ from the pinned package manifest')
record = dict(release=args.ref, source_commit=commit, date='2026-10-11',
              reference_runtime='Node v26.8.1', profile='native-dev', host_mode='--node',
              result_path=result_path, methodology_path='conformance/npm-compat/README.md',
              packages=packages)
(root / 'data/npm-compat.json').write_text(json.dumps(record, indent=2) + '\n')
print(f'Imported {len(packages)} pinned npm package results')
