#!/usr/bin/env python3
"""Run pinned upstream Effect tests as TypeScript, with Bun as the reference."""
import argparse
import concurrent.futures
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parent.parent
HARNESS = REPO / 'conformance/effect-upstream'
PIN = json.loads((HARNESS / 'corpus.json').read_text())
PREFIX = 'KILN_EFFECT '


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def command(args, cwd):
    subprocess.run(args, cwd=cwd, check=True)


def prepare(workspace):
    source = workspace / 'source'
    if not source.exists():
        command(['git', 'clone', '--filter=blob:none', '--no-checkout', PIN['repository'], str(source)], workspace)
        command(['git', 'checkout', '--detach', PIN['commit']], source)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
    if revision != PIN['commit']:
        raise RuntimeError(f'upstream checkout is {revision}; expected {PIN["commit"]}')
    if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=no'], cwd=source, text=True).strip():
        raise RuntimeError('upstream tracked files have changed')
    corpus = source / PIN['directory']
    actual = sorted(str(p.relative_to(corpus))[:-len('.test.ts')] for p in corpus.rglob('*.test.ts'))
    if not actual or actual != PIN['files']:
        raise RuntimeError('upstream test inventory differs from committed pin')
    # One owned workspace contains both sources and dependencies, so compiler project
    # boundaries and module identity agree across the runtimes.
    for path in HARNESS.iterdir():
        if path.is_file() and path.name != 'corpus.json':
            shutil.copyfile(path, workspace / path.name)
    lock_digest = digest(workspace / 'package-lock.json')
    stamp = workspace / '.dependencies'
    if not (workspace / 'node_modules').exists() or not stamp.exists() or stamp.read_text() != lock_digest:
        command(['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'], workspace)
        stamp.write_text(lock_digest)
    links = {
        source / 'node_modules': workspace / 'node_modules',
        workspace / 'node_modules/effect': source / 'packages/effect',
        workspace / 'node_modules/@effect/vitest': source / 'packages/vitest',
        workspace / 'node_modules/@effect/opentelemetry': source / 'packages/opentelemetry',
    }
    for link, target in links.items():
        if link.is_symlink():
            link.unlink()
        elif link.exists():
            if link.is_dir():
                shutil.rmtree(link)
            else:
                link.unlink()
        link.parent.mkdir(parents=True, exist_ok=True)
        link.symlink_to(target, target_is_directory=True)
    paths = {
        'vitest': [str(workspace / 'vitest-shim.mjs')],
        'effect': [str(source / 'packages/effect/src/index.ts')],
        'effect/*': [str(source / 'packages/effect/src/*')],
        '@effect/vitest': [str(source / 'packages/vitest/src/index.ts')],
        '@effect/vitest/utils': [str(source / 'packages/vitest/src/utils.ts')],
        '@effect/opentelemetry/*': [str(source / 'packages/opentelemetry/src/*')],
        'effect-test/*': [str(corpus / '*')],
    }
    # Exact package export entries take precedence over the wildcard: effect/schema
    # is schema/index.ts, not schema.ts. Read the pinned owner instead of duplicating
    # its directory list in this runner.
    for package in ('effect', 'opentelemetry'):
        directory = source / 'packages' / package
        metadata = json.loads((directory / 'package.json').read_text())
        for export, target in metadata['exports'].items():
            if isinstance(target, str):
                name = metadata['name'] + (export[1:] if export != '.' else '')
                paths[name] = [str(directory / target)]
    write_json(workspace / 'tsconfig.json', {'compilerOptions': {'baseUrl': str(workspace), 'paths': paths}})
    return source


def entry_for(workspace, source, name):
    test = source / PIN['directory'] / (name + '.test.ts')
    text = test.read_text()
    directory = workspace / 'files' / name
    directory.mkdir(parents=True, exist_ok=True)
    # The official upstream setup installs Effect equality testers, so expect is
    # unconditional. Optional adapters add their real library implementations.
    features = ['expect']
    import re
    if re.search(r'\b(?:vi|vitest)\s*\.', text):
        features.append('spy')
    if re.search(r'\b(?:useFakeTimers|useRealTimers|advanceTimersByTime)', text):
        features.append('timers')
    if 'toMatchInlineSnapshot' in text:
        features.append('snapshot')
    imports = ''.join('import ' + json.dumps(str(workspace / (f + '.mjs'))) + ';\n' for f in features)
    entry = directory / 'entry.ts'
    entry.write_text(imports + 'import ' + json.dumps(str(source / 'vitest.setup.ts')) + ';\n'
                     + 'import ' + json.dumps(str(test)) + ';\n'
                     + 'import {run} from ' + json.dumps(str(workspace / 'harness.mjs')) + ';\n'
                     + 'run().catch(error=>{console.error("KILN_EFFECT "+JSON.stringify({kind:"runner_error",error:String(error),stack:error.stack}));process.exitCode=1;});\n')
    # kiln resolves aliases from the entry project; Bun receives the exact same config.
    shutil.copyfile(workspace / 'tsconfig.json', directory / 'tsconfig.json')
    return entry


def execute(args, cwd, log, timeout, runtime_jobs):
    started = time.monotonic()
    timed_out = False
    with log.open('wb') as output:
        process = subprocess.Popen(args, cwd=cwd, env={**os.environ, 'TZ': 'UTC', 'KILN_JOBS': str(runtime_jobs), 'RAYON_NUM_THREADS': str(runtime_jobs)},
                                   stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            code = process.wait()
    return {'command': args, 'exit_code': code, 'timed_out': timed_out,
            'wall_s': time.monotonic() - started, 'log': str(log)}


def records(log):
    result = []
    malformed = []
    for line in log.read_text(errors='replace').splitlines():
        if line.startswith(PREFIX):
            try:
                result.append(json.loads(line[len(PREFIX):]))
            except json.JSONDecodeError:
                malformed.append({'kind': 'runner_error', 'error': 'malformed result marker', 'line': line})
    inventory = [r['tests'] for r in result if r['kind'] == 'inventory']
    summaries = [r for r in result if r['kind'] == 'summary']
    cases = [r for r in result if r['kind'] == 'case']
    errors = malformed + [r for r in result if r['kind'] in ('hook_error', 'runner_error')]
    ids = [r['id'] for r in cases]
    valid = (len(inventory) == 1 and len(summaries) == 1
             and len(ids) == len(set(ids)) and ids == [r['id'] for r in inventory[0]]
             and len(ids) > 0 and not errors)
    summary = summaries[0] if len(summaries) == 1 else None
    if valid:
        counts = {outcome: sum(r['outcome'] == outcome for r in cases) for outcome in ('pass', 'fail', 'skip')}
        valid = (summary['discovered'] == len(ids) and summary['passed'] == counts['pass']
                 and summary['failed'] == counts['fail'] and summary['skipped'] == counts['skip'])
    return {'inventory': inventory[0] if len(inventory) == 1 else None,
            'cases': cases, 'summary': summary, 'errors': errors, 'complete': valid}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, default=Path.home() / '.cache/kiln' / ('effect-direct-' + PIN['commit'][:12]))
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--engine', action='append', choices=['bun', 'interpreter', 'native'])
    parser.add_argument('--path', action='append', help='test stem or directory prefix (repeatable)')
    parser.add_argument('--kiln', type=Path, default=REPO / 'target/release/kiln')
    parser.add_argument('--jobs', type=int, default=min(2, max(1, int(os.environ.get('KILN_JOBS', '2')))))
    parser.add_argument('--runtime-jobs', type=int, default=1, help='compiler/interpreter workers within each process')
    parser.add_argument('--timeout', type=float, default=180)
    parser.add_argument('--kiln-memory-mb', type=int, default=8192, help='hard address-space limit per kiln process (children inherit)')
    args = parser.parse_args()
    if args.jobs < 1 or args.timeout <= 0 or args.kiln_memory_mb < 1 or args.runtime_jobs < 1:
        parser.error('jobs and timeout must be positive')
    workspace = args.workspace.resolve()
    workspace.mkdir(parents=True, exist_ok=True)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    engines = args.engine or ['bun', 'native']
    if len(set(engines)) != len(engines):
        parser.error('engines must be distinct')
    selected = [name for name in PIN['files'] if not args.path or any(name == p or name.startswith(p.rstrip('/') + '/') for p in args.path)]
    if not selected:
        parser.error('no pinned tests matched')
    kiln = args.kiln.resolve()
    with (workspace / '.runner.lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        source = prepare(workspace)
        entries = {name: entry_for(workspace, source, name) for name in selected}
        versions = {}
        for engine in engines:
            cmd = ['bun', '--version'] if engine == 'bun' else [str(kiln), '--version']
            versions[engine] = subprocess.check_output(cmd, text=True).strip()
        report = {'schema': 1, 'pin': PIN['commit'], 'input_mode': 'direct-typescript',
                  'reference': 'bun', 'versions': versions, 'selected': len(selected),
                  'deferred': len(PIN['files']) - len(selected), 'jobs': args.jobs,
                  'kiln_memory_mb': args.kiln_memory_mb, 'timeout_s': args.timeout, 'runtime_jobs': args.runtime_jobs,
                  'harness_sha256': {p.name: digest(p) for p in HARNESS.iterdir() if p.is_file()},
                  'source_sha256': {n: digest(source / PIN['directory'] / (n + '.test.ts')) for n in selected},
                  'files': []}
        def run(name):
            directory = out / name
            directory.mkdir(parents=True, exist_ok=True)
            row = {'file': name, 'engines': {}}
            for engine in engines:
                if engine == 'bun':
                    cmd = ['bun', '--tsconfig-override', str(workspace / 'tsconfig.json'), str(entries[name])]
                else:
                    cmd = [str(kiln), 'run', str(entries[name]), '--node']
                    if engine == 'native':
                        cmd += ['--backend', 'native-dev']
                    cmd = [sys.executable, str(Path(__file__).resolve()), '--limited-child', str(args.kiln_memory_mb), *cmd]
                log = directory / (engine + '.log')
                envelope = execute(cmd, workspace, log, args.timeout, args.runtime_jobs)
                envelope.update(records(log))
                envelope['complete'] &= not envelope['timed_out']
                row['engines'][engine] = envelope
                write_json(directory / (engine + '.json'), envelope)
            reference = row['engines'].get('bun')
            for engine, envelope in row['engines'].items():
                envelope['matches_reference_inventory'] = (envelope['inventory'] == reference['inventory']) if reference else None
                envelope['matches_reference_outcomes'] = ([{k: c[k] for k in ('id', 'outcome')} for c in envelope['cases']] == [{k: c[k] for k in ('id', 'outcome')} for c in reference['cases']]) if reference else None
            return row
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = {pool.submit(run, name): name for name in selected}
            for future in concurrent.futures.as_completed(futures):
                row = future.result()
                report['files'].append(row)
                report['files'].sort(key=lambda r: r['file'])
                write_json(out / 'report.json', report)
                print(row['file'], {e: r['summary'] if r['complete'] else 'INCOMPLETE' for e, r in row['engines'].items()}, flush=True)
        clean = all(r['complete'] and r['exit_code'] == 0 and r['summary']['failed'] == 0
                    and r['matches_reference_inventory'] is not False
                    for row in report['files'] for r in row['engines'].values())
        report['passed'] = clean
        write_json(out / 'report.json', report)
        return 0 if clean else 1


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--limited-child':
        import resource
        limit = int(sys.argv[2]) * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
        os.execvp(sys.argv[3], sys.argv[3:])
    sys.exit(main())
