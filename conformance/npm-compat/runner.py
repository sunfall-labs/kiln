#!/usr/bin/env python3
"""Package parity evidence; durations are phase wall time, not warmed workload benchmarks."""
import argparse
import csv
import datetime
import hashlib
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import time

HERE = Path(__file__).resolve().parent


def command(argv, prefix, timeout):
    started = time.perf_counter()
    with prefix.with_suffix('.stdout').open('wb') as out, prefix.with_suffix('.stderr').open('wb') as err:
        process = subprocess.Popen(argv, cwd=HERE, stdout=out, stderr=err, start_new_session=True)
        timed_out = False
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            # Include compiler and host children: an orphan can keep consuming the shared budget.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            code = process.wait()
        except BaseException:
            # Cancellation also owns the compiler/runtime children in this process group.
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
            except ProcessLookupError:
                process.wait()
            raise
    return {'argv': list(map(str, argv)), 'exit': code, 'timeout': timed_out,
            'seconds': round(time.perf_counter() - started, 6)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kiln', required=True)
    parser.add_argument('--raw', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=600)
    parser.add_argument('--profile', choices=('native-dev', 'native-release'), default='native-dev',
                        help='native build profile; phase timings retain this policy explicitly')
    parser.add_argument('packages', nargs='*')
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error('--timeout must be positive')
    kiln = str(Path(args.kiln).resolve())
    # Commands execute from the corpus directory; keep artifacts anchored to the caller.
    args.raw = args.raw.resolve()
    rows = []
    with (HERE / 'packages.tsv').open() as f:
        for row in csv.reader(f, delimiter='\t'):
            if row:
                if len(row) != 5:
                    parser.error(f'invalid package row: {row!r}')
                rows.append(row)
    names = [row[0] for row in rows]
    if len(names) != len(set(names)) or len(names) != 361:
        parser.error('inventory must contain exactly 361 unique packages')
    unknown = sorted(set(args.packages) - set(names))
    if unknown:
        parser.error('unknown packages: ' + ', '.join(unknown))
    by_name = {row[0]: row for row in rows}
    # Explicit slices follow the requested order, so small discriminators can precede large
    # compiler bundles. Deduplicate filters without silently changing full-corpus order.
    selected = [by_name[name] for name in dict.fromkeys(args.packages)] if args.packages else rows
    tests = {}
    for name, version, porffor, detail, stem in rows:
        matches = [p for ext in ('js', 'mjs') if (p := HERE / 'tests' / f'{stem}.test.{ext}').is_file()]
        if len(matches) != 1:
            parser.error(f'{name}: expected exactly one smoke test, found {len(matches)}')
        tests[name] = matches[0]
    args.raw.mkdir(parents=True, exist_ok=True)
    evidence = {'schema': 1, 'generated': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'state': 'running', 'completed': 0,
                'inventory_sha256': hashlib.sha256((HERE / 'packages.tsv').read_bytes()).hexdigest(),
                'kiln_sha256': hashlib.sha256(Path(kiln).read_bytes()).hexdigest(),
                'selected': len(selected), 'deferred': len(rows) - len(selected),
                'discovered': len(rows),
                'timezone': os.environ.get('TZ'),
                'host': 'node',
                'execution_engine': 'native',
                'native_profile': args.profile,
                'kiln_jobs': os.environ.get('KILN_JOBS'),
                'timing_kind': 'phase wall time including startup; not warmed workload throughput',
                'results': []}
    evidence['git_head'] = subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=HERE, text=True).strip()
    evidence['node_version'] = subprocess.check_output(['node', '--version'], text=True).strip()
    def interrupt_run(signum, frame):
        # SIGTERM from a supervising agent follows the same cleanup/evidence path as Ctrl-C.
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupt_run)
    try:
        for name, version, porffor, _, stem in selected:
            test = str(tests[name])
            # with_suffix replaces the last suffix, so use an additional terminal suffix.
            node = args.raw / f'{stem}.node.log'
            build = args.raw / f'{stem}.build.log'
            native = args.raw / f'{stem}.kiln.log'
            binary = args.raw / f'{stem}.bin'
            result = {'name': name, 'version': version, 'porffor_status': porffor,
                      'test_sha256': hashlib.sha256(tests[name].read_bytes()).hexdigest(), 'phases': {}}
            phases = result['phases']
            phases['node'] = command(['node', test], node, 60)
            if phases['node']['exit'] != 0 or phases['node']['timeout']:
                status = 'node-baseline-failed'
            else:
                phases['build'] = command([kiln, '--one-shot', 'build', test, '--node', '--profile', args.profile, '--output', str(binary)], build, args.timeout)
                summary = build.with_suffix('.stdout').read_text(errors='replace')
                timing = re.search(r'emit (\d+)ms, compile (\d+)ms, link (\d+)ms', summary)
                if timing:
                    phases['build']['native_timing_ms'] = dict(zip(('emit', 'compile', 'link'), map(int, timing.groups())))
                if phases['build']['timeout']:
                    status = 'compile-timeout'
                elif phases['build']['exit'] != 0 or not binary.is_file():
                    status = 'compile-error'
                else:
                    result['binary_bytes'] = binary.stat().st_size
                    phases['native'] = command([str(binary)], native, args.timeout)
                    if phases['native']['timeout']:
                        status = 'runtime-timeout'
                    elif phases['native']['exit'] != 0:
                        status = 'runtime-error'
                    elif node.with_suffix('.stdout').read_bytes() == native.with_suffix('.stdout').read_bytes():
                        status = 'pass'
                    else:
                        status = 'wrong-output'
            result['status'] = status
            evidence['results'].append(result)
            evidence['completed'] = len(evidence['results'])
            # Persist after every package so interruption does not lose completed evidence.
            (args.raw / 'results.json').write_text(json.dumps(evidence, indent=2) + '\n')
            timing = ', '.join(f'{phase}={data["seconds"]:.3f}s' for phase, data in phases.items())
            print(f'{name}@{version} -> {status} ({timing})', flush=True)
    except KeyboardInterrupt:
        evidence['state'] = 'interrupted'
        (args.raw / 'results.json').write_text(json.dumps(evidence, indent=2) + '\n')
        print(f'Interrupted after {evidence["completed"]}/{len(selected)} packages; evidence: {args.raw}', flush=True)
        raise
    counts = {}
    tsv = args.raw / 'results.tsv'
    with tsv.open('w') as f:
        writer = csv.writer(f, delimiter='\t', lineterminator='\n')
        writer.writerow(['name', 'version', 'kiln_status', 'porffor_status', 'build_s', 'native_s', 'node_s'])
        for result in evidence['results']:
            phases = result['phases']
            counts[result['status']] = counts.get(result['status'], 0) + 1
            writer.writerow([result['name'], result['version'], result['status'], result['porffor_status'],
                             *(phases.get(phase, {}).get('seconds', '') for phase in ('build', 'native', 'node'))])
    evidence['counts'] = counts
    evidence['state'] = 'complete'
    (args.raw / 'results.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print(f'coverage: selected={len(selected)}, deferred={len(rows)-len(selected)}, discovered={len(rows)}')
    print(f'{counts.get("pass", 0)}/{len(selected)} pass; evidence: {args.raw}')
    # Package failures remain informational; malformed inventories fail before execution.


if __name__ == '__main__':
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(130)
