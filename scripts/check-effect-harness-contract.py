#!/usr/bin/env python3
"""Compare the direct-TS adapter's deadline/cleanup contract to pinned Vitest.

Use a workspace prepared by effect-upstream.py (its pinned dependencies are required).
Every fixture invocation deliberately contains failures; case records are the verdict.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = [
    ('hung cleanup', 'fail'),
    ('cleanup continues', 'pass'),
    ('hook budget > hook does not spend body budget', 'pass'),
    ('sync overrun', 'fail'),
    ('undefined throw', 'fail'),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', required=True, type=Path)
    parser.add_argument('--kiln', type=Path)
    parser.add_argument('--native', action='store_true')
    args = parser.parse_args()
    workspace = args.workspace.resolve()
    vitest = workspace / 'node_modules/vitest/vitest.mjs'
    if not vitest.is_file():
        parser.error('workspace must contain the pinned Effect/Vitest dependencies')
    if args.native and not args.kiln:
        parser.error('--native requires --kiln')
    destination = workspace / '.effect-harness-contract'
    destination.mkdir(exist_ok=True)
    for source in (ROOT / 'scripts/fixtures/effect-harness-contract').iterdir():
        shutil.copy2(source, destination / source.name)
    shutil.copy2(ROOT / 'conformance/effect-upstream/harness.mjs', destination / 'harness.mjs')

    def run(name, command, expected_exit=None):
        with (destination / (name + '.log')).open('w') as output:
            result = subprocess.run(command, cwd=workspace, stdout=output,
                                    stderr=subprocess.STDOUT, timeout=300)
        if expected_exit is not None and result.returncode != expected_exit:
            raise RuntimeError(f'{name}: unexpected exit {result.returncode}; see {destination}')
        return destination / (name + '.log')

    if args.kiln:
        # A requested wall-clock policy is insufficient: older CLIs answered ClockNow with a
        # synthetic 2020 epoch even under --clock wall. Such a CLI can pass the adapter's
        # self-clocked spin fixture while failing to charge synchronous work real milliseconds.
        # Reject that known failure before comparing deadline outcomes to Vitest.
        probe = destination / 'clock-epoch.mjs'
        probe.write_text('console.log("KILN_CLOCK_EPOCH " + JSON.stringify(Date.now()));\n')
        started_ms = time.time() * 1000
        log = run('interpreter-clock', [str(args.kiln.resolve()), '--one-shot', 'run',
                                      str(probe), '--node', '--clock', 'wall'], 0)
        finished_ms = time.time() * 1000
        samples = [json.loads(line.removeprefix('KILN_CLOCK_EPOCH '))
                   for line in log.read_text().splitlines()
                   if line.startswith('KILN_CLOCK_EPOCH ')]
        tolerance_ms = 1000
        if (len(samples) != 1 or type(samples[0]) not in (int, float)
                or not started_ms - tolerance_ms <= samples[0] <= finished_ms + tolerance_ms):
            raise RuntimeError('interpreter --clock wall did not return the system epoch; '
                               f'samples={samples!r}; see {log}')
        (destination / 'clock-epoch.json').write_text(json.dumps({
            'check': 'system epoch sanity; deadline behavior is checked separately',
            'sample_ms': samples[0], 'parent_started_ms': started_ms,
            'parent_finished_ms': finished_ms, 'tolerance_ms': tolerance_ms,
        }, indent=2) + '\n')

    oracle = destination / 'oracle.json'
    oracle.unlink(missing_ok=True)
    run('vitest', ['node', str(vitest), 'run', '--config',
                  str(destination / 'vitest.config.mjs'), '--reporter=json',
                  '--outputFile', str(oracle)], 1)
    report = json.loads(oracle.read_text())
    cases = [( ' > '.join(case['ancestorTitles'] + [case['title']]),
               {'passed': 'pass', 'failed': 'fail'}[case['status']])
             for suite in report['testResults'] for case in suite['assertionResults']]
    if cases != EXPECTED:
        raise RuntimeError(f'Vitest contract changed: {cases!r}')

    def check(name, command):
        log = run(name, command)
        records = [json.loads(line.removeprefix('KILN_EFFECT '))
                   for line in log.read_text().splitlines() if line.startswith('KILN_EFFECT ')]
        cases = [(case['id'], case['outcome']) for case in records if case['kind'] == 'case']
        summaries = [case for case in records if case['kind'] == 'summary']
        inventories = [case for case in records if case['kind'] == 'inventory']
        if (cases != EXPECTED or len(summaries) != 1 or len(inventories) != 1
                or inventories[0]['tests'] != [{'id': name, 'mode': 'run'} for name, _ in EXPECTED]
                or summaries[0] != {'kind': 'summary', 'discovered': 5, 'passed': 2,
                                    'failed': 3, 'skipped': 0}):
            raise RuntimeError(f'{name}: contract mismatch; see {log}')
        print(f'{name}: 5/5 outcomes match pinned Vitest')

    shim = str(destination / 'shim.mjs')
    check('node', ['node', shim])
    check('bun', ['bun', shim])
    if args.kiln:
        kiln = str(args.kiln.resolve())
        check('interpreter', [kiln, '--one-shot', 'run', shim, '--node', '--clock', 'wall'])
        if args.native:
            binary = destination / 'native'
            binary.unlink(missing_ok=True)
            run('native-build', [kiln, '--one-shot', 'build', shim, '--node',
                                 '--profile', 'native-release', '--output', str(binary)], 0)
            check('native', [str(binary)])
    print(f'evidence: {destination}')


if __name__ == '__main__':
    main()
