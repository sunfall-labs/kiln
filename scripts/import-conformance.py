#!/usr/bin/env python3
"""Snapshot conformance baselines from an immutable source-repository revision."""
import argparse
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
