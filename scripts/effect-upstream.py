#!/usr/bin/env python3
"""Run pinned Effect tests as TypeScript with separate Bun and Node/Vitest oracles."""
import argparse
import concurrent.futures
import fcntl
import hashlib
import json
import os
import re
from collections import Counter
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parent.parent
HARNESS = REPO / 'conformance/effect-upstream'
PIN = json.loads((HARNESS / 'corpus.json').read_text())
RUNTIME_TARGETS = json.loads((HARNESS / 'runtime-targets.json').read_text())
ENGINE_MODE_POLICY = json.loads((HARNESS / 'engine-mode-policy.json').read_text())
NODE_VITEST_ORACLE_PATH = HARNESS / 'node-vitest-reference.json'
NODE_VITEST_ORACLE = json.loads(NODE_VITEST_ORACLE_PATH.read_text())
NODE_VITEST_MAPPINGS_PATH = HARNESS / 'node-vitest-case-mapping.json'
NODE_VITEST_MAPPINGS = json.loads(NODE_VITEST_MAPPINGS_PATH.read_text())
NODE_VITEST_GAPS_PATH = HARNESS / 'node-vitest-comparison-gaps.json'
NODE_VITEST_GAPS = json.loads(NODE_VITEST_GAPS_PATH.read_text())
PREFIX = 'KILN_EFFECT '


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


NODE_VITEST_ORACLE_SHA256 = digest(NODE_VITEST_ORACLE_PATH)


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + '.partial')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)



def snapshot_provider(manifest_path, toolchain):
    """Own both provider inputs; never link from regenerable or mutable source outputs."""
    original = manifest_path.resolve()
    manifest_bytes = original.read_bytes()
    manifest = json.loads(manifest_bytes)
    archive_name = manifest.get('archive')
    if (not isinstance(archive_name, str) or not archive_name
            or archive_name in ('.', '..') or Path(archive_name).name != archive_name
            or '/' in archive_name or '\\' in archive_name):
        raise RuntimeError('provider archive must be one file name')
    archive = original.parent / archive_name
    archive_sha = digest(archive)
    if archive_sha != manifest.get('archive_sha256'):
        raise RuntimeError('provider archive digest does not match manifest')
    manifest_sha = hashlib.sha256(manifest_bytes).hexdigest()
    destination = toolchain / ('provider-' + manifest_sha)
    if not destination.exists():
        temporary = Path(tempfile.mkdtemp(prefix='.provider-', dir=toolchain))
        try:
            shutil.copy2(archive, temporary / archive_name)
            if digest(temporary / archive_name) != archive_sha:
                raise RuntimeError('provider archive changed while taking the snapshot')
            (temporary / 'kiln-native-provider.json').write_bytes(manifest_bytes)
            try:
                temporary.rename(destination)
            except FileExistsError:
                pass
        finally:
            if temporary.exists():
                shutil.rmtree(temporary)
    snapshot = destination / 'kiln-native-provider.json'
    if digest(snapshot) != manifest_sha or digest(destination / archive_name) != archive_sha:
        raise RuntimeError('corrupt provider snapshot: ' + str(destination))
    return {'manifest': str(snapshot), 'manifest_sha256': manifest_sha,
            'archive_sha256': archive_sha, 'source_sha256': manifest.get('source_sha256'),
            'provider_abi': manifest.get('provider_abi'), 'rustc_identity': manifest.get('rustc_identity'),
            'rust_target': manifest.get('rust_target')}


def prepare_provider(explicit, kiln_source, out, timeout, runtime_jobs, memory_mb):
    candidates = [explicit, os.environ.get('KILN_NATIVE_PROVIDER_MANIFEST'),
                  os.environ.get('KILN_NATIVE_NUMBER_PROVIDER_MANIFEST')]
    supplied = next((Path(p) for p in candidates if p is not None), None)
    if supplied is None:
        supplied = next((p for p in (kiln_source.parent / 'kiln-native-provider.json',
                                    kiln_source.parent.parent / 'lib/kiln/kiln-native-provider.json')
                         if p.is_file()), None)
    preparation = None
    if supplied is None:
        package = out / 'provider-package'
        # Source fallback is assembled once before the run, under the pinned toolchain.
        # Every child then consumes the owned archive, regardless of later source edits.
        command = [sys.executable, str(Path(__file__).resolve()), '--limited-child', str(memory_mb),
                   'mise', 'exec', '--', str(REPO / 'scripts/package-native-number-provider.sh'),
                   str(package)]
        preparation = execute(command, REPO, out / 'provider-preparation.log',
                              max(300, timeout), runtime_jobs)
        if preparation['exit_code'] != 0:
            raise RuntimeError('provider preparation failed; see ' + preparation['log'])
        supplied = package / 'kiln-native-provider.json'
    snapshot = snapshot_provider(supplied, out / 'toolchain')
    snapshot['original_manifest'] = str(supplied.resolve())
    snapshot['preparation'] = preparation
    return snapshot

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
    # Entry directories belong to the runner; resolution authority remains the
    # shared workspace, including when a previous preparation left a manifest.
    (directory / 'package.json').unlink(missing_ok=True)
    # The official upstream setup installs Effect equality testers, so expect is
    # unconditional. Optional adapters add their real library implementations.
    features = ['expect']
    if re.search(r'\brequire\s*\(', text):
        targets = RUNTIME_TARGETS['require'].get(name)
        if not targets:
            raise RuntimeError(f'{name}: require targets are not declared')
        # These original literal targets stay lazy. A finite adapter-owned dispatch
        # makes the native linker retain them without evaluating them during setup.
        # The genuine createRequire resolves each against the original test file.
        loaders = ',\n'.join(json.dumps(target) + ':()=>require(' + json.dumps(target) + ')'
                             for target in targets)
        (directory / 'node-support.mjs').write_text(
            "import {createRequire} from 'node:module';\n"
            + 'const require=createRequire(' + json.dumps(test.as_uri()) + ');\n'
            + 'const loaders={' + loaders + '};\n'
            + 'globalThis.require=specifier=>Object.hasOwn(loaders,specifier)?loaders[specifier]():require(specifier);\n')
        features.insert(0, 'node-support')
    if re.search(r'\b(?:vi|vitest)\s*\.', text):
        # Keep the original test URL as the loader's reload authority even though the
        # separate spy adapter registers its own canonical URL for namespace overlays.
        (directory / 'spy-support.mjs').write_text(
            'const loader=globalThis.process?.__kilnNativeModules?.__kilnHostModuleLoader;\n'
            + 'if(typeof loader==="function")loader("registerRunner",' + json.dumps(test.as_uri()) + ');\n')
        features.extend(['spy-support', 'spy'])
    if re.search(r'\b(?:useFakeTimers|useRealTimers|advanceTimersByTime)', text):
        features.append('timers')
    if 'toMatchInlineSnapshot' in text:
        features.append('snapshot')
    if re.search(r'\b(?:vi|vitest)\s*\.\s*(?:resetModules|importActual)', text):
        # Module reset is an explicit test-loader policy, scoped to the original
        # test referrer. Literal lazy roots retain each importActual target in the
        # native graph; they are never called just to register the capability.
        targets = RUNTIME_TARGETS['importActual'].get(name, [])
        registry = ',\n'.join(
            json.dumps(specifier) + ':{url:' + json.dumps((test.parent / specifier).resolve().as_uri())
            + ',load:()=>import(' + json.dumps(str((test.parent / specifier).resolve())) + ')}'
            for specifier in targets)
        for specifier in targets:
            if not (test.parent / specifier).resolve().is_file():
                raise RuntimeError(f'{name}: importActual target {specifier} is missing')
        (directory / 'reload-support.mjs').write_text(
            'import {installVi} from ' + json.dumps(str(workspace / 'harness.mjs')) + ';\n'
            + 'const referrer=' + json.dumps(test.as_uri()) + ';\n'
            + 'const targets={' + registry + '};\n'
            + 'const loader=process?.__kilnNativeModules?.__kilnHostModuleLoader;\n'
            + 'if(typeof loader==="function"){\n'
            + 'loader("registerRunner",referrer);\n'
            + 'installVi({resetModules(){loader("reset");},importActual(specifier){\n'
            + 'if(!Object.hasOwn(targets,specifier))throw new Error("HARNESS_UNSUPPORTED: undeclared importActual target "+specifier);\n'
            + 'return Promise.resolve(loader("importActual",specifier,referrer,targets[specifier].url));\n'
            + '}});\n}\n')
        features.append('reload-support')
    imports = ''.join('import ' + json.dumps(str((directory if f in ('node-support', 'reload-support', 'spy-support') else workspace) / (f + '.mjs'))) + ';\n' for f in features)
    entry = directory / 'entry.ts'
    entry.write_text(imports + 'import ' + json.dumps(str(source / 'vitest.setup.ts')) + ';\n'
                     + 'import ' + json.dumps(str(test)) + ';\n'
                     + 'import {run} from ' + json.dumps(str(workspace / 'harness.mjs')) + ';\n'
                     + 'run().catch(error=>{console.error("KILN_EFFECT "+JSON.stringify({kind:"runner_error",error:String(error),stack:error.stack}));process.exitCode=1;});\n')
    # kiln resolves aliases from the entry project; Bun receives the exact same config.
    shutil.copyfile(workspace / 'tsconfig.json', directory / 'tsconfig.json')
    return entry


def execute(args, cwd, log, timeout, runtime_jobs, extra_env=None):
    started = time.monotonic()
    timed_out = False
    with log.open('wb') as output:
        process = subprocess.Popen(args, cwd=cwd, env={**os.environ, 'TZ': 'UTC', 'KILN_JOBS': str(runtime_jobs), 'RAYON_NUM_THREADS': str(runtime_jobs), **(extra_env or {})},
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


def aggregate(report, engines):
    """Keep partial coverage and runner failures visible beside case totals."""
    totals = {}
    for engine in engines:
        rows = [row['engines'][engine] for row in report['files']]
        totals[engine] = {
            'complete_files': sum(row['complete'] for row in rows),
            'incomplete_files': sum(not row['complete'] for row in rows),
            'registered_cases': sum(len(row['inventory'] or []) for row in rows),
            'recorded_cases': sum(len(row['cases']) for row in rows),
            'partial_cases': sum(len(row['cases']) for row in rows if not row['complete']),
            **{outcome: sum(case['outcome'] == outcome for row in rows for case in row['cases'])
               for outcome in ('pass', 'fail', 'skip')},
            'timed_out_files': sum(row['timed_out'] for row in rows),
            'nonzero_exit_files': sum(row['exit_code'] != 0 for row in rows),
            'reference_inventory_mismatches': sum(row['matches_reference_inventory'] is False for row in rows),
            'reference_mode_mismatches': sum(row['matches_reference_modes'] is False for row in rows),
            'reference_outcome_mismatches': sum(row['matches_reference_outcomes'] is False for row in rows),
            'engine_conditional_mode_cases': sum(len(row.get('engine_conditional_mode_cases', [])) for row in rows),
            'node_vitest_mapped_cases': sum((row.get('node_vitest') or {}).get('mapped_cases', 0) for row in rows),
            'node_vitest_unmapped_cases': sum(len((row.get('node_vitest') or {}).get('unmapped_cases', [])) for row in rows),
            'node_vitest_official_unmapped_cases': sum(len((row.get('node_vitest') or {}).get('official_unmapped_cases', [])) for row in rows),
            'node_vitest_mapping_errors': sum(len((row.get('node_vitest') or {}).get('mapping_errors', [])) for row in rows),
            'node_vitest_mode_mismatches': sum(len((row.get('node_vitest') or {}).get('mode_mismatches', [])) for row in rows),
            'node_vitest_outcome_mismatches': sum(len((row.get('node_vitest') or {}).get('outcome_mismatches', [])) for row in rows),
            'node_vitest_known_source_transform_gaps': sum(len((row.get('node_vitest') or {}).get('known_source_transform_gaps', [])) for row in rows),
            'bun_adapter_unsupported_cases': sum(len(row.get('bun_adapter_unsupported_cases', [])) for row in rows),
        }
    return {'reported_files': len(report['files']),
            'pending_files': report['selected'] - len(report['files']),
            'deferred_files': report['deferred'], 'engines': totals}


def comparison_verdicts(report, engines):
    rows = report['files']
    selected_files = report.get('selected_files')
    selected_set = set(selected_files) if isinstance(selected_files, list) else None
    reported_files = [row.get('file') for row in rows]
    coverage_errors = []
    if selected_set is None:
        coverage_errors.append('selected_files_missing')
    else:
        if len(selected_files) != len(selected_set):
            coverage_errors.append('selected_files_contains_duplicates')
        if len(reported_files) != len(set(reported_files)):
            coverage_errors.append('runtime_report_contains_duplicate_files')
        missing = sorted(selected_set - set(reported_files))
        unexpected = sorted(set(reported_files) - selected_set)
        if missing:
            coverage_errors.append('selected_files_missing_from_report')
        if unexpected:
            coverage_errors.append('unselected_files_present_in_report')
    result = {'node_vitest_conformance': {}, 'direct_bun_differential': {},
              'bun_adapter_unsupported': []}
    for engine in ('native', 'interpreter'):
        if engine not in engines:
            result['node_vitest_conformance'][engine] = {'status': 'not_run'}
            continue
        comparisons = [row['engines'][engine].get('node_vitest', {}) for row in rows
                       if engine in row['engines']]
        if not comparisons:
            requested = isinstance(selected_files, list) and bool(selected_files)
            result['node_vitest_conformance'][engine] = {
                'status': 'incomplete' if requested else 'not_run',
                'mapped_cases': 0, 'unmapped_direct_cases': 0, 'unmatched_official_cases': 0,
                'mode_mismatches': 0, 'outcome_mismatches': 0,
                'runtime_incomplete_files': sorted(selected_set) if requested else [],
                'oracle_incomplete_files': sorted(selected_set) if requested else [],
                'coverage_errors': ['runtime_records_missing'] if requested else []}
            continue
        compared_rows = [row for row in rows if engine in row.get('engines', {})]
        runtime_incomplete = [row.get('file') for row in compared_rows
                              if row['engines'][engine].get('complete') is not True]
        oracle_incomplete = [row.get('file') for row in compared_rows
                             if row['engines'][engine].get('node_vitest', {}).get('complete') is not True]
        expected_rows = len(selected_files) if isinstance(selected_files, list) else len(rows)
        incomplete = (bool(coverage_errors) or len(compared_rows) != expected_rows or
                      bool(runtime_incomplete) or bool(oracle_incomplete) or
                      any(c.get('complete') is not True for c in comparisons))
        mode_mismatches = sum(len(c.get('mode_mismatches', [])) for c in comparisons)
        outcome_mismatches = sum(len(c.get('outcome_mismatches', [])) for c in comparisons)
        mapped = sum(c.get('mapped_cases', 0) for c in comparisons)
        unmapped = sum(len(c.get('unmapped_cases', [])) for c in comparisons)
        official_unmapped = sum(len(c.get('official_unmapped_cases', [])) for c in comparisons)
        status = 'incomplete' if incomplete else ('fail' if mode_mismatches or outcome_mismatches else 'pass')
        result['node_vitest_conformance'][engine] = {
            'status': status, 'mapped_cases': mapped, 'unmapped_direct_cases': unmapped,
            'unmatched_official_cases': official_unmapped, 'mode_mismatches': mode_mismatches,
            'outcome_mismatches': outcome_mismatches,
            'runtime_incomplete_files': sorted(set(runtime_incomplete)),
            'oracle_incomplete_files': sorted(set(oracle_incomplete)),
            'coverage_errors': coverage_errors}
    if 'bun' in engines and any(engine in engines for engine in ('native', 'interpreter')):
        candidates = [
            (engine, row['engines'][engine])
            for engine in ('native', 'interpreter') for row in rows if engine in row['engines']]
        if not candidates:
            result['direct_bun_differential'] = {'status': 'not_run'}
            return result
        inv_mismatches = sum(candidate.get('matches_reference_inventory') is False for _, candidate in candidates)
        mode_mismatches = sum(candidate.get('matches_reference_modes') is False for _, candidate in candidates)
        outcome_mismatches = sum(candidate.get('matches_reference_outcomes') is False for _, candidate in candidates)
        unknown = any(candidate.get('matches_reference_inventory') is None or
                      candidate.get('matches_reference_modes') is None or
                      candidate.get('matches_reference_outcomes') is None for _, candidate in candidates)
        status = 'diverged' if inv_mismatches or mode_mismatches or outcome_mismatches else ('incomplete' if unknown else 'match')
        result['direct_bun_differential'] = {'status': status,
                                             'inventory_mismatches': inv_mismatches,
                                             'mode_mismatches': mode_mismatches,
                                             'outcome_mismatches': outcome_mismatches}
    result['bun_adapter_unsupported'] = [
        {'file': row['file'], **case}
        for row in rows for case in row['engines'].get('bun', {}).get('bun_adapter_unsupported_cases', [])]
    return result


def compare_reference(reference, candidate, engine, file_name):
    """Compare common cases to Bun, honoring only explicitly declared engine modes."""
    reference_inventory = reference.get('inventory') if reference else None
    candidate_inventory = candidate.get('inventory')
    reference_cases = reference.get('cases') if reference else None
    candidate_cases = candidate.get('cases')
    if (reference_inventory is None or candidate_inventory is None or
            reference_cases is None or candidate_cases is None):
        return {'inventory': None, 'modes': None, 'outcomes': None, 'conditional_modes': []}
    inventory_matches = [case['id'] for case in candidate_inventory] == [case['id'] for case in reference_inventory]
    if not inventory_matches:
        return {'inventory': False, 'modes': False, 'outcomes': False, 'conditional_modes': []}
    reference_ids = [case['id'] for case in reference_inventory]
    candidate_ids = [case['id'] for case in candidate_inventory]
    if ([case['id'] for case in reference_cases] != reference_ids or
            [case['id'] for case in candidate_cases] != candidate_ids):
        return {'inventory': True, 'modes': False, 'outcomes': False, 'conditional_modes': [],
                'mode_mismatches': [], 'outcome_mismatches': [{'id': 'case-record-order'}]}

    policies = {case['id']: case for case in ENGINE_MODE_POLICY['cases'] if case['file'] == file_name}
    mode_mismatches = []
    outcome_mismatches = []
    reference_mode_mismatches = []
    candidate_mode_mismatches = []
    reference_outcome_mismatches = []
    candidate_outcome_mismatches = []
    conditional_modes = []
    for reference_mode, candidate_mode, reference_case, candidate_case in zip(
            reference_inventory, candidate_inventory, reference_cases, candidate_cases):
        policy = policies.get(candidate_mode['id'])
        if policy is not None:
            expected_reference_mode = policy['modes'].get('bun')
            expected_candidate_mode = policy['modes'].get(engine)
            expected_reference_outcome = policy['outcomes'].get('bun')
            expected_candidate_outcome = policy['outcomes'].get(engine)
            if reference_mode['mode'] != expected_reference_mode:
                detail = {'id': candidate_mode['id'], 'actual': reference_mode['mode'],
                          'expected': expected_reference_mode}
                mode_mismatches.append(detail)
                reference_mode_mismatches.append(detail)
            if reference_case['outcome'] != expected_reference_outcome:
                detail = {'id': candidate_mode['id'], 'actual': reference_case['outcome'],
                          'expected': expected_reference_outcome}
                outcome_mismatches.append(detail)
                reference_outcome_mismatches.append(detail)
            if candidate_mode['mode'] != expected_candidate_mode:
                detail = {'id': candidate_mode['id'], 'actual': candidate_mode['mode'],
                          'expected': expected_candidate_mode}
                mode_mismatches.append(detail)
                candidate_mode_mismatches.append(detail)
            if candidate_case['outcome'] != expected_candidate_outcome:
                detail = {'id': candidate_mode['id'], 'actual': candidate_case['outcome'],
                          'expected': expected_candidate_outcome}
                outcome_mismatches.append(detail)
                candidate_outcome_mismatches.append(detail)
            if (reference_mode['mode'] != expected_reference_mode or
                    reference_case['outcome'] != expected_reference_outcome or
                    candidate_mode['mode'] != expected_candidate_mode or
                    candidate_case['outcome'] != expected_candidate_outcome):
                continue
            if reference_mode['mode'] != candidate_mode['mode']:
                conditional_modes.append({'id': candidate_mode['id'], 'bun_mode': reference_mode['mode'],
                                          'engine': engine, 'engine_mode': candidate_mode['mode'],
                                          'expected_outcome': expected_candidate_outcome,
                                          'reason': policy['reason']})
            continue
        if reference_mode['mode'] == candidate_mode['mode']:
            if reference_case['outcome'] != candidate_case['outcome']:
                detail = {'id': candidate_mode['id'], 'reference': reference_case['outcome'],
                          'actual': candidate_case['outcome']}
                outcome_mismatches.append(detail)
                candidate_outcome_mismatches.append(detail)
            continue
        detail = {'id': candidate_mode['id'], 'reference': reference_mode['mode'],
                  'actual': candidate_mode['mode']}
        mode_mismatches.append(detail)
        reference_mode_mismatches.append(detail)
        candidate_mode_mismatches.append(detail)
        if reference_case['outcome'] != candidate_case['outcome']:
            detail = {'id': candidate_mode['id'], 'reference': reference_case['outcome'],
                      'actual': candidate_case['outcome']}
            outcome_mismatches.append(detail)
            candidate_outcome_mismatches.append(detail)
    return {'inventory': True, 'modes': not mode_mismatches, 'outcomes': not outcome_mismatches,
            'mode_mismatches': mode_mismatches, 'outcome_mismatches': outcome_mismatches,
            'reference_mode_mismatches': reference_mode_mismatches,
            'candidate_mode_mismatches': candidate_mode_mismatches,
            'reference_outcome_mismatches': reference_outcome_mismatches,
            'candidate_outcome_mismatches': candidate_outcome_mismatches,
            'conditional_modes': conditional_modes}


def node_vitest_case_map(file_name, inventory, source_sha256):
    """Map direct-source cases to official Vitest cases without positional fallback."""
    oracle_file = NODE_VITEST_ORACLE['files'].get(file_name)
    if oracle_file is None:
        return {'complete': False, 'mapped': [], 'unmapped': [
            {'id': case.get('id'), 'reason': 'file_absent_from_node_oracle'} for case in inventory or []],
            'official_unmapped': [], 'mapping_errors': ['file absent from pinned Node/Vitest oracle']}
    if source_sha256 != oracle_file['source_sha256']:
        return {'complete': False, 'mapped': [], 'unmapped': [
            {'id': case.get('id'), 'reason': 'source_digest_mismatch'} for case in inventory or []],
            'official_unmapped': [case['id'] for case in oracle_file['cases']],
            'mapping_errors': [{'expected': oracle_file['source_sha256'], 'actual': source_sha256}]}

    exact = {}
    for index, case in enumerate(oracle_file['cases']):
        exact.setdefault(case['id'], []).append(index)
    explicit = [row for row in NODE_VITEST_MAPPINGS['mappings'] if row['file'] == file_name]
    explicit_by_id = {}
    mapping_errors = []
    for row in explicit:
        if row['source_sha256'] != source_sha256 or row['direct_id'] in explicit_by_id:
            mapping_errors.append({'direct_id': row['direct_id'], 'reason': 'stale_or_duplicate_mapping'})
        explicit_by_id[row['direct_id']] = row

    selected = set()
    mapped = []
    unmapped = []
    direct_ids = [case.get('id') for case in inventory or []]
    if len(direct_ids) != len(set(direct_ids)):
        mapping_errors.append({'reason': 'direct_inventory_contains_duplicate_ids'})
    for direct in inventory or []:
        direct_id = direct.get('id')
        candidates = exact.get(direct_id, [])
        node_index = candidates[0] if len(candidates) == 1 else None
        explicit_row = explicit_by_id.get(direct_id)
        if explicit_row is not None:
            matches = exact.get(explicit_row['official_id'], [])
            occurrence = explicit_row.get('official_occurrence')
            if not explicit_row.get('source_line') or not explicit_row.get('evidence'):
                mapping_errors.append({'direct_id': direct_id, 'reason': 'mapping_lacks_source_evidence'})
            if len(candidates) == 1:
                mapping_errors.append({'direct_id': direct_id, 'reason': 'explicit_override_of_unique_exact_match'})
            if not isinstance(occurrence, int) or occurrence < 0 or occurrence >= len(matches):
                mapping_errors.append({'direct_id': direct_id, 'reason': 'invalid_official_occurrence'})
            else:
                node_index = matches[occurrence]
        if node_index is None:
            unmapped.append({'id': direct_id, 'reason': 'missing_exact_or_explicit_mapping',
                             'official_candidates': len(candidates)})
            continue
        if node_index in selected:
            mapping_errors.append({'direct_id': direct_id, 'reason': 'non_one_to_one_mapping'})
            unmapped.append({'id': direct_id, 'reason': 'non_one_to_one_mapping',
                             'official_candidates': len(candidates)})
            continue
        selected.add(node_index)
        mapped.append({'direct_id': direct_id, 'official_index': node_index,
                       'official': oracle_file['cases'][node_index]})
    official_unmapped = [{'index': i, 'id': case['id']} for i, case in enumerate(oracle_file['cases'])
                         if i not in selected]
    return {'complete': not unmapped and not official_unmapped and not mapping_errors,
            'mapped': mapped, 'unmapped': unmapped, 'official_unmapped': official_unmapped,
            'mapping_errors': mapping_errors}


def compare_node_vitest(candidate, file_name, source_sha256):
    inventory = candidate.get('inventory')
    cases = candidate.get('cases')
    if inventory is None or cases is None:
        return {'applicable': True, 'complete': False, 'matches_modes': None,
                'matches_outcomes': None, 'mapped_cases': 0, 'unmapped_cases': [],
                'official_unmapped_cases': [], 'mapping_errors': ['candidate inventory unavailable'],
                'mode_mismatches': [], 'outcome_mismatches': [], 'known_source_transform_gaps': []}
    mapping = node_vitest_case_map(file_name, inventory, source_sha256)
    envelope_errors = []
    ids = [row.get('id') for row in inventory]
    case_ids = [row.get('id') for row in cases]
    if candidate.get('complete') is not True:
        envelope_errors.append('runtime_envelope_incomplete')
    if candidate.get('errors'):
        envelope_errors.append('runtime_envelope_contains_errors')
    if candidate.get('timed_out') is True or candidate.get('matches_expected_exit') is False:
        envelope_errors.append('runtime_execution_did_not_complete_cleanly')
    if len(ids) != len(set(ids)) or len(case_ids) != len(set(case_ids)):
        envelope_errors.append('duplicate_runtime_case_ids')
    if ids != case_ids:
        envelope_errors.append('runtime_inventory_and_case_records_differ')
    summary = candidate.get('summary')
    if not isinstance(summary, dict):
        envelope_errors.append('runtime_summary_missing')
    else:
        actual_counts = {outcome: sum(row.get('outcome') == outcome for row in cases)
                         for outcome in ('pass', 'fail', 'skip')}
        expected_summary = {'discovered': len(ids), 'passed': actual_counts['pass'],
                            'failed': actual_counts['fail'], 'skipped': actual_counts['skip']}
        if any(summary.get(key) != value for key, value in expected_summary.items()):
            envelope_errors.append('runtime_summary_does_not_match_case_records')
    candidate_by_id = {case.get('id'): case for case in cases}
    mode_by_id = {case.get('id'): case.get('mode') for case in inventory}
    mode_mismatches = []
    outcome_mismatches = []
    gap_ids = {(row['file'], row['id']): row for row in NODE_VITEST_GAPS['cases']}
    known_gaps = []
    for pair in mapping['mapped']:
        direct_id = pair['direct_id']
        official = pair['official']
        actual = candidate_by_id.get(direct_id)
        if actual is None:
            outcome_mismatches.append({'id': direct_id, 'expected': official['outcome'],
                                       'actual': 'missing_case_record'})
            continue
        if mode_by_id.get(direct_id) != official['mode']:
            mode_mismatches.append({'id': direct_id, 'expected': official['mode'],
                                    'actual': mode_by_id.get(direct_id)})
        if actual.get('outcome') != official['outcome']:
            detail = {'id': direct_id, 'expected': official['outcome'], 'actual': actual.get('outcome')}
            gap = gap_ids.get((file_name, direct_id))
            if gap is not None:
                detail['classification'] = gap['classification']
                detail['reason'] = gap['reason']
                known_gaps.append(detail)
            outcome_mismatches.append(detail)
    complete = mapping['complete'] and not envelope_errors
    return {'applicable': True, 'complete': complete,
            'matches_modes': not mode_mismatches if complete else None,
            'matches_outcomes': not outcome_mismatches if complete else None,
            'mapped_cases': len(mapping['mapped']), 'unmapped_cases': mapping['unmapped'],
            'official_unmapped_cases': mapping['official_unmapped'],
            'mapping_errors': mapping['mapping_errors'], 'envelope_errors': envelope_errors,
            'mode_mismatches': mode_mismatches, 'outcome_mismatches': outcome_mismatches,
            'known_source_transform_gaps': known_gaps}


def bun_adapter_unsupported(cases):
    """Keep adapter omissions distinct from Bun runtime outcomes and upstream skips."""
    return [{'id': case['id'], 'error': case.get('error')}
            for case in cases or []
            if isinstance(case.get('error'), str) and 'HARNESS_UNSUPPORTED:' in case['error']]


def validate_node_vitest_policy():
    if NODE_VITEST_ORACLE.get('schema') != 1 or NODE_VITEST_ORACLE.get('pin') != PIN['commit']:
        raise ValueError('Node/Vitest oracle schema or Effect pin mismatch')
    summary = NODE_VITEST_ORACLE.get('summary', {})
    oracle_files = NODE_VITEST_ORACLE.get('files', {})
    if set(oracle_files) != set(PIN['files']):
        raise ValueError('Node/Vitest oracle coverage does not match the pinned full corpus')
    all_cases = [case for file in oracle_files.values() for case in file.get('cases', [])]
    actual_totals = {'files': len(oracle_files), 'registered': len(all_cases),
                     'pass': sum(case.get('outcome') == 'pass' for case in all_cases),
                     'fail': sum(case.get('outcome') == 'fail' for case in all_cases),
                     'skip': sum(case.get('outcome') == 'skip' for case in all_cases)}
    per_file_totals = {}
    for name, file in oracle_files.items():
        cases = file.get('cases', [])
        per_file_totals[name] = {'registered': len(cases), **{
            outcome: sum(case.get('outcome') == outcome for case in cases)
            for outcome in ('pass', 'fail', 'skip')}}
    if summary.get('per_file_totals') != per_file_totals:
        raise ValueError('Node/Vitest per-file totals do not match their case records')
    if any(summary.get(key) != value for key, value in actual_totals.items()):
        raise ValueError('Node/Vitest archive summary does not match its case records')
    if actual_totals != {'files': len(PIN['files']), 'registered': 11019,
                         'pass': 10974, 'fail': 0, 'skip': 45}:
        raise ValueError('Node/Vitest oracle totals do not match the pinned full corpus')
    if NODE_VITEST_MAPPINGS.get('schema') != 1 or NODE_VITEST_MAPPINGS.get('oracle_sha256') != NODE_VITEST_ORACLE_SHA256:
        raise ValueError('Node/Vitest explicit mapping file does not match the oracle digest')
    if NODE_VITEST_GAPS.get('schema') != 1 or NODE_VITEST_GAPS.get('oracle_sha256') != NODE_VITEST_ORACLE_SHA256:
        raise ValueError('Node/Vitest comparability-gap file does not match the oracle digest')
    for name, file in oracle_files.items():
        if len(file.get('source_sha256', '')) != 64:
            raise ValueError('Node/Vitest oracle has invalid source digest for ' + name)
        cases = file.get('cases', [])
        ids = [case.get('id') for case in cases]
        if not cases or any(not isinstance(identifier, str) or not identifier for identifier in ids):
            raise ValueError('Node/Vitest oracle has missing case identity for ' + name)
        if any(case.get('mode') not in ('run', 'skip') or case.get('outcome') not in ('pass', 'fail', 'skip')
               for case in cases):
            raise ValueError('Node/Vitest oracle has invalid mode/outcome for ' + name)
    unresolved = NODE_VITEST_ORACLE.get('unmapped_direct_cases', [])
    categories = NODE_VITEST_ORACLE.get('summary', {}).get('unmapped_categories', {})
    unresolved_keys = [(row.get('file'), row.get('direct_id')) for row in unresolved]
    if len(unresolved_keys) != len(set(unresolved_keys)):
        raise ValueError('Node/Vitest archive repeats an unresolved direct case')
    reason_counts = Counter(row.get('reason') for row in unresolved)
    reason_files = {reason: len({row.get('file') for row in unresolved if row.get('reason') == reason})
                    for reason in reason_counts}
    if (summary.get('unmapped_reason_counts') != dict(sorted(reason_counts.items())) or
            summary.get('unmapped_reason_files') != dict(sorted(reason_files.items()))):
        raise ValueError('Node/Vitest unmapped reason summary does not match its case records')
    category_counts = {}
    for category in ('parameterized_title_label', 'occurrence_disambiguated_label', 'ambiguous_official_title'):
        rows = [row for row in unresolved if
                (('[case ' in row['direct_id']) if category == 'parameterized_title_label' else
                 ('[occurrence ' in row['direct_id']) if category == 'occurrence_disambiguated_label' else
                 (row['reason'] == 'ambiguous_official_title'))]
        category_counts[category] = {'cases': len(rows), 'files': len({row['file'] for row in rows})}
        if categories.get(category) != category_counts[category]:
            raise ValueError('Node/Vitest unmapped-case category totals are inconsistent: ' + category)
        for row in rows:
            if row.get('source_sha256') != NODE_VITEST_ORACLE['files'].get(row['file'], {}).get('source_sha256'):
                raise ValueError('unmapped direct case has stale source digest: ' + repr(row['direct_id']))
    if sum(category_counts[k]['cases'] for k in category_counts) != summary.get('unmapped_direct_cases'):
        raise ValueError('Node/Vitest unmapped-case total is inconsistent')
    unique_official_ids = sum(count == 1 for file in NODE_VITEST_ORACLE['files'].values()
                              for count in Counter(c['id'] for c in file['cases']).values())
    parameterized_mappings = sum('[case ' in row.get('direct_id', '')
                                 for row in NODE_VITEST_MAPPINGS['mappings'])
    auto = unique_official_ids - (category_counts['parameterized_title_label']['cases'] +
                                  parameterized_mappings)
    if auto != summary.get('automatic_exact_unique_mappings'):
        raise ValueError('Node/Vitest automatic mapping total is inconsistent')
    seen = set()
    seen_targets = set()
    for row in NODE_VITEST_MAPPINGS['mappings']:
        key = (row.get('file'), row.get('direct_id'))
        if key in seen:
            raise ValueError('duplicate explicit Node/Vitest mapping: ' + repr(key))
        seen.add(key)
        if row.get('source_sha256') != NODE_VITEST_ORACLE.get('files', {}).get(row.get('file'), {}).get('source_sha256'):
            raise ValueError('explicit Node/Vitest mapping has stale source digest: ' + repr(key))
        if not row.get('evidence') or not isinstance(row.get('source_line'), int) or row['source_line'] < 1:
            raise ValueError('explicit Node/Vitest mapping lacks source evidence: ' + repr(key))
        oracle_file = oracle_files.get(row.get('file'))
        candidates = [case for case in oracle_file.get('cases', [])
                      if case.get('id') == row.get('official_id')]
        occurrence = row.get('official_occurrence')
        parameterized = '[case ' in row.get('direct_id', '')
        if parameterized:
            indices = re.findall(r'\[case (\d+)\]', row['direct_id'])
            if (not indices or not isinstance(row.get('parameter_index'), int) or
                    row['parameter_index'] != int(indices[-1]) or
                    not isinstance(row.get('parameter_source_line'), int) or row['parameter_source_line'] < 1 or
                    not isinstance(row.get('parameter_value'), str) or not row['parameter_value']):
                raise ValueError('parameterized mapping lacks exact source argument evidence: ' + repr(key))
        if (not candidates or not isinstance(occurrence, int) or
                occurrence < 0 or occurrence >= len(candidates)):
            raise ValueError('explicit Node/Vitest mapping has no valid official occurrence: ' + repr(key))
        target = (row['file'], row['official_id'], occurrence)
        if target in seen_targets:
            raise ValueError('explicit Node/Vitest mappings reuse an official occurrence: ' + repr(target))
        seen_targets.add(target)
    gap_keys = set()
    for row in NODE_VITEST_GAPS.get('cases', []):
        key = (row.get('file'), row.get('id'))
        if key in gap_keys:
            raise ValueError('duplicate Node/Vitest source-transformation gap: ' + repr(key))
        gap_keys.add(key)
        oracle_file = NODE_VITEST_ORACLE['files'].get(row.get('file'))
        if (oracle_file is None or oracle_file.get('source_sha256') != row.get('source_sha256') or
                not any(case['id'] == row.get('id') for case in oracle_file['cases']) or
                not row.get('classification') or not row.get('reason') or
                not isinstance(row.get('source_line'), int) or row['source_line'] < 1):
            raise ValueError('invalid Node/Vitest source-transformation gap: ' + repr(key))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, default=Path.home() / '.cache/kiln' / ('effect-direct-' + PIN['commit'][:12]))
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--engine', action='append', choices=['bun', 'interpreter', 'native'])
    parser.add_argument('--path', action='append', default=[], help='test stem or directory prefix (repeatable)')
    parser.add_argument('--kiln', type=Path, default=REPO / 'target/release/kiln')
    parser.add_argument('--provider-manifest', type=Path, help='native provider to snapshot; defaults to an installed package or one built from this repository')
    parser.add_argument('--jobs', type=int, default=min(2, max(1, int(os.environ.get('KILN_JOBS', '2')))))
    parser.add_argument('--runtime-jobs', type=int, default=1, help='compiler/interpreter workers within each process')
    parser.add_argument('--native-profile', choices=['native-dev', 'native-release'], default='native-dev',
                        help='native compilation profile (recorded in the report; case timeouts are unchanged)')
    parser.add_argument('--timeout', type=float, default=180)
    parser.add_argument('--kiln-memory-mb', type=int, default=8192, help='hard address-space limit per kiln process (children inherit)')
    args = parser.parse_args()
    try:
        validate_node_vitest_policy()
    except ValueError as exc:
        parser.error(str(exc))
    if args.jobs < 1 or args.timeout <= 0 or args.kiln_memory_mb < 1 or args.runtime_jobs < 1:
        parser.error('jobs and timeout must be positive')
    workspace = args.workspace.resolve()
    workspace.mkdir(parents=True, exist_ok=True)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    engines = args.engine or ['bun', 'native']
    if len(set(engines)) != len(engines):
        parser.error('engines must be distinct')
    unmatched = [p for p in args.path if not any(name == p or name.startswith(p.rstrip('/') + '/') for name in PIN['files'])]
    if unmatched:
        parser.error('unknown pinned test selectors: ' + ', '.join(unmatched))
    selected = [name for name in PIN['files'] if not args.path or any(name == p or name.startswith(p.rstrip('/') + '/') for p in args.path)]
    if not selected:
        parser.error('no pinned tests matched')
    kiln_source = args.kiln.resolve()
    kiln = kiln_source
    kiln_sha = None
    if any(engine != 'bun' for engine in engines):
        # mbx targets are shared, regenerable build outputs. A long corpus run owns its
        # executable snapshot so cache reclamation cannot delete its future launchers.
        kiln_sha = digest(kiln_source)
        toolchain = out / 'toolchain'
        toolchain.mkdir(exist_ok=True)
        kiln = toolchain / ('kiln-' + kiln_sha)
        if not kiln.exists():
            temporary = toolchain / ('.kiln-' + str(os.getpid()))
            try:
                shutil.copy2(kiln_source, temporary)
                if digest(temporary) != kiln_sha:
                    raise RuntimeError('kiln executable changed while taking the run snapshot')
                temporary.replace(kiln)
            finally:
                temporary.unlink(missing_ok=True)
        if digest(kiln) != kiln_sha:
            raise RuntimeError('corrupt kiln executable snapshot: ' + str(kiln))
    provider = prepare_provider(args.provider_manifest, kiln_source, out, args.timeout, args.runtime_jobs, args.kiln_memory_mb) if 'native' in engines else None
    native_env = {'KILN_NATIVE_PROVIDER_MANIFEST': provider['manifest']} if provider else {}
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
                  'selected_files': selected,
                  'node_vitest_oracle': {
                      'file': str(NODE_VITEST_ORACLE_PATH.relative_to(REPO)),
                      'sha256': NODE_VITEST_ORACLE_SHA256,
                      'official_report_sha256': NODE_VITEST_ORACLE['provenance']['official_report_sha256'],
                      'node': NODE_VITEST_ORACLE['provenance']['node'],
                      'vitest': NODE_VITEST_ORACLE['provenance']['vitest'],
                      'case_mapping': 'unique exact direct-source ID or a versioned source-anchored one-to-one exception; no positional fallback',
                      'mapping_file': str(NODE_VITEST_MAPPINGS_PATH.relative_to(REPO)),
                      'mapping_sha256': digest(NODE_VITEST_MAPPINGS_PATH),
                      'gaps_file': str(NODE_VITEST_GAPS_PATH.relative_to(REPO)),
                      'gaps_sha256': digest(NODE_VITEST_GAPS_PATH),
                  },
                  'engine_mode_policy': {'file': 'conformance/effect-upstream/engine-mode-policy.json',
                                         'sha256': digest(HARNESS / 'engine-mode-policy.json'),
                                         'cases': ENGINE_MODE_POLICY['cases']},
                  'clock_policy': {e: 'wall' for e in engines},
                  'native_profile': args.native_profile if 'native' in engines else None,
                  'native_provider': provider,
                  'kiln_binary_sha256': kiln_sha,
                  'kiln_binary_source': str(kiln_source) if kiln_sha else None,
                  'kiln_binary_snapshot': str(kiln) if kiln_sha else None,
                  'runner_sha256': digest(Path(__file__).resolve()),
                  'runner_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
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
                    cmd = ['bun', '--preload', str(workspace / 'bun-support.mjs'), '--tsconfig-override', str(workspace / 'tsconfig.json'), str(entries[name])]
                else:
                    limiter = [sys.executable, str(Path(__file__).resolve()), '--limited-child', str(args.kiln_memory_mb)]
                    cmd = [*limiter, str(kiln), '--one-shot', 'run', str(entries[name]), '--node', '--clock', 'wall']
                build = None
                if engine == 'native':
                    # A shipped executable has the host-clock policy. Native run's
                    # deterministic clock cannot time out continuously runnable work.
                    binary = directory / 'native.bin'
                    binary.unlink(missing_ok=True)
                    build_cmd = [*limiter, str(kiln), '--one-shot', 'build', str(entries[name]),
                                 '--node', '--profile', args.native_profile, '--output', str(binary)]
                    for specifier in RUNTIME_TARGETS.get('include', {}).get(name, []):
                        if specifier.startswith('.'):
                            specifier = str((source / PIN['directory'] / (name + '.test.ts')).parent.joinpath(specifier).resolve())
                        build_cmd.extend(['--include', specifier])
                    build = execute(build_cmd, workspace, directory / 'native-build.log', args.timeout, args.runtime_jobs, native_env)
                    if build['exit_code'] != 0 or not binary.is_file():
                        envelope = {**build, 'build': build, 'inventory': None, 'cases': [],
                                    'summary': None, 'errors': [{'kind': 'runner_error', 'error': 'native build failed'}], 'complete': False}
                        row['engines'][engine] = envelope
                        write_json(directory / (engine + '.json'), envelope)
                        continue
                    cmd = [*limiter, str(binary)]
                log = directory / (engine + '.log')
                envelope = execute(cmd, workspace, log, args.timeout, args.runtime_jobs,
                                   native_env if engine != 'bun' else None)
                if build is not None:
                    envelope['build'] = build
                    envelope['binary_sha256'] = digest(binary)
                envelope.update(records(log))
                summary = envelope['summary']
                expected_exit = (1 if summary['failed'] else 0) if summary else None
                envelope['matches_expected_exit'] = envelope['exit_code'] == expected_exit
                # A crash after the summary is an incomplete execution, even when
                # every recorded case passed. Failed assertions normally exit 1.
                envelope['complete'] &= not envelope['timed_out'] and envelope['matches_expected_exit']
                row['engines'][engine] = envelope
                write_json(directory / (engine + '.json'), envelope)
            reference = row['engines'].get('bun')
            for engine, envelope in row['engines'].items():
                comparison = compare_reference(reference, envelope, engine, name) if reference else {
                    'inventory': None, 'modes': None, 'outcomes': None, 'conditional_modes': []}
                envelope['matches_reference_inventory'] = comparison['inventory']
                envelope['matches_reference_modes'] = comparison['modes']
                envelope['matches_reference_outcomes'] = comparison['outcomes']
                envelope['reference_mode_mismatches'] = comparison.get('reference_mode_mismatches', [])
                envelope['candidate_mode_mismatches'] = comparison.get('candidate_mode_mismatches', [])
                envelope['reference_outcome_mismatches'] = comparison.get('reference_outcome_mismatches', [])
                envelope['candidate_outcome_mismatches'] = comparison.get('candidate_outcome_mismatches', [])
                envelope['engine_conditional_mode_cases'] = comparison['conditional_modes']
                if engine == 'bun':
                    envelope['node_vitest'] = {'applicable': False,
                                               'reason': 'Bun is the direct-runtime differential reference, not the native conformance oracle.'}
                    envelope['bun_adapter_unsupported_cases'] = bun_adapter_unsupported(envelope.get('cases'))
                else:
                    envelope['node_vitest'] = compare_node_vitest(
                        envelope, name, report['source_sha256'].get(name))
                write_json(out / name / (engine + '.json'), envelope)
            return row
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
            futures = {pool.submit(run, name): name for name in selected}
            for future in concurrent.futures.as_completed(futures):
                row = future.result()
                report['files'].append(row)
                report['files'].sort(key=lambda r: r['file'])
                report['aggregate'] = aggregate(report, engines)
                write_json(out / 'report.json', report)
                print(row['file'], {e: r['summary'] if r['complete'] else 'INCOMPLETE' for e, r in row['engines'].items()}, flush=True)
        clean = all(r['complete'] and r['exit_code'] == 0 and r['summary']['failed'] == 0
                    and r['matches_reference_inventory'] is not False
                    and r['matches_reference_modes'] is not False
                    and r['matches_reference_outcomes'] is not False
                    and (engine == 'bun' or (
                        r.get('node_vitest', {}).get('complete') is True
                        and r.get('node_vitest', {}).get('matches_modes') is True
                        and r.get('node_vitest', {}).get('matches_outcomes') is True))
                    for row in report['files'] for engine, r in row['engines'].items())
        report['passed'] = clean and len(report['files']) == len(selected) and \
            {row['file'] for row in report['files']} == set(selected)
        report['comparisons'] = comparison_verdicts(report, engines)
        write_json(out / 'report.json', report)
        print('aggregate', json.dumps(report['aggregate'], sort_keys=True), flush=True)
        return 0 if clean else 1


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--limited-child':
        import resource
        limit = int(sys.argv[2]) * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
        os.execvp(sys.argv[3], sys.argv[3:])
    sys.exit(main())
