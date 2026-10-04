import importlib.util
import copy
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location(
    'effect_upstream', Path(__file__).with_name('effect-upstream.py'))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


CASE_ID = 'Cache > does not retain prior equal object keys on cache hits'


def envelope(engine, mode, outcome):
    return {
        'inventory': [{'id': CASE_ID, 'mode': mode}],
        'cases': [{'id': CASE_ID, 'outcome': outcome}],
    }


class EngineModeComparisonTests(unittest.TestCase):
    def test_declared_bun_gc_skip_is_compared_to_node_expected_kiln_pass(self):
        result = runner.compare_reference(
            envelope('bun', 'skip', 'skip'), envelope('native', 'run', 'pass'),
            'native', 'Cache')
        self.assertTrue(result['inventory'])
        self.assertTrue(result['modes'])
        self.assertTrue(result['outcomes'])
        self.assertEqual(len(result['conditional_modes']), 1)

    def test_unexpected_mode_or_outcome_is_not_exempted(self):
        result = runner.compare_reference(
            envelope('bun', 'skip', 'skip'), envelope('native', 'skip', 'skip'),
            'native', 'Cache')
        self.assertFalse(result['modes'])
        self.assertFalse(result['outcomes'])
        self.assertEqual(result['conditional_modes'], [])

    def test_expected_mode_with_failed_gc_outcome_is_only_an_outcome_mismatch(self):
        result = runner.compare_reference(
            envelope('bun', 'skip', 'skip'), envelope('native', 'run', 'fail'),
            'native', 'Cache')
        self.assertTrue(result['modes'])
        self.assertFalse(result['outcomes'])
        self.assertEqual(result['reference_mode_mismatches'], [])
        self.assertEqual(result['candidate_mode_mismatches'], [])
        self.assertEqual(len(result['candidate_outcome_mismatches']), 1)
        self.assertEqual(result['conditional_modes'], [])

    def test_reference_policy_deviation_has_orthogonal_diagnostics(self):
        result = runner.compare_reference(
            envelope('bun', 'run', 'fail'), envelope('native', 'run', 'pass'),
            'native', 'Cache')
        self.assertFalse(result['modes'])
        self.assertFalse(result['outcomes'])
        self.assertEqual(len(result['reference_mode_mismatches']), 1)
        self.assertEqual(len(result['reference_outcome_mismatches']), 1)
        self.assertEqual(result['candidate_mode_mismatches'], [])
        self.assertEqual(result['candidate_outcome_mismatches'], [])

    def test_undeclared_mode_difference_stays_a_mismatch(self):
        other_id = 'Cache > unrelated case'
        reference = {'inventory': [{'id': other_id, 'mode': 'skip'}],
                     'cases': [{'id': other_id, 'outcome': 'skip'}]}
        candidate = {'inventory': [{'id': other_id, 'mode': 'run'}],
                     'cases': [{'id': other_id, 'outcome': 'pass'}]}
        result = runner.compare_reference(reference, candidate, 'native', 'Cache')
        self.assertFalse(result['modes'])
        self.assertFalse(result['outcomes'])

    def test_all_ten_gc_guarded_cases_have_exact_source_provenance(self):
        policies = runner.ENGINE_MODE_POLICY['cases']
        self.assertEqual(len(policies), 10)
        self.assertEqual(len({(case['file'], case['id']) for case in policies}), 10)
        for case in policies:
            self.assertEqual(len(case['source_sha256']), 64)
            self.assertGreater(case['source_line'], 0)
            self.assertIn(case['guard_form'], ('it.effect.skipIf', 'it.skipIf', 'describe.skipIf'))
            self.assertIn('process.versions.bun', case['source_guard'])
            self.assertEqual(case['modes'], {'bun': 'skip', 'interpreter': 'run', 'native': 'run'})
            self.assertEqual(case['outcomes'], {'bun': 'skip', 'interpreter': 'pass', 'native': 'pass'})
            self.assertIn('official Node/Vitest', case['reason'])
            self.assertEqual(case['official_node_vitest'], {'mode': 'run', 'outcome': 'pass'})


class NativeResourcePolicyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='effect-runner-resource-test-')
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def fake_zstd(self, body):
        executable = self.root / 'zstd-fixture'
        executable.write_text('#!/usr/bin/env python3\n' + body)
        executable.chmod(0o755)
        return executable

    def test_resource_flags_are_off_by_default_and_opt_in(self):
        defaults = runner.build_parser().parse_args(['--out', str(self.root / 'out')])
        enabled = runner.build_parser().parse_args([
            '--out', str(self.root / 'out'), '--compress-native-artifacts', '--cache-gc'])
        self.assertFalse(defaults.compress_native_artifacts)
        self.assertFalse(defaults.cache_gc)
        self.assertTrue(enabled.compress_native_artifacts)
        self.assertTrue(enabled.cache_gc)

    def test_cache_gc_uses_first_writable_shared_layer_and_same_gc_root(self):
        workspace = self.root / 'workspace'
        workspace.mkdir()
        provider = {'manifest': '/owned/provider.json'}
        default_env = runner.native_resource_env(provider, workspace, False)
        shared_env = runner.native_resource_env(provider, workspace, True)
        self.assertNotIn('KILN_SHARED_CACHE', default_env)
        self.assertEqual(shared_env['KILN_SHARED_CACHE'], str(workspace / '.kiln/cache'))
        log = self.root / 'gc.log'
        with mock.patch.object(runner, 'execute', return_value={'exit_code': 0, 'timed_out': False,
                                                                 'wall_s': 0.1, 'log': str(log)}) as execute:
            result = runner.run_cache_gc(Path('/owned/kiln'), workspace, log, 1, 8192)
        _, cwd, _, _, _ = execute.call_args.args
        self.assertEqual(Path(cwd) / '.kiln/cache', Path(shared_env['KILN_SHARED_CACHE']))
        self.assertEqual(result['project_context'], str(workspace))

    def test_compression_verifies_archive_before_removing_raw_binary(self):
        zstd = self.fake_zstd('''import pathlib,sys
args=sys.argv[1:]
if '--version' in args:
 print('zstd fixture'); raise SystemExit(0)
data=pathlib.Path(args[-1]).read_bytes()
sys.stdout.buffer.write(data)
''')
        binary = self.root / 'native.bin'
        binary.write_bytes(b'native artifact payload' * 100)
        original_sha = runner.digest(binary)
        result = runner.compress_native_artifact(binary, zstd, original_sha)
        self.assertEqual(result['status'], 'verified')
        self.assertEqual(result['original_sha256'], original_sha)
        self.assertEqual(result['archive_sha256'], runner.digest(Path(result['archive'])))
        self.assertEqual(Path(result['archive']).read_bytes(), b'native artifact payload' * 100)
        self.assertEqual(result['verification'], 'streamed_sha256_and_size')
        self.assertFalse(binary.exists())

    def test_compression_failure_preserves_raw_binary_and_reports_reason(self):
        zstd = self.fake_zstd('''import pathlib,sys
args=sys.argv[1:]
if '-d' in args:
 data=pathlib.Path(args[-1]).read_bytes(); sys.stdout.buffer.write(data)
else:
 print('injected compression failure', file=sys.stderr); raise SystemExit(7)
''')
        binary = self.root / 'native.bin'
        binary.write_bytes(b'keep me')
        with self.assertRaisesRegex(runner.NativeArtifactError, 'exited 7.*injected compression failure'):
            runner.compress_native_artifact(binary, zstd)
        self.assertEqual(binary.read_bytes(), b'keep me')
        self.assertFalse((self.root / 'native.bin.zst').exists())
        self.assertFalse((self.root / 'native.bin.zst.partial').exists())

    def test_round_trip_mismatch_preserves_raw_binary(self):
        zstd = self.fake_zstd('''import pathlib,sys
args=sys.argv[1:]
data=pathlib.Path(args[-1]).read_bytes()
sys.stdout.buffer.write(data + (b'corrupt' if '-d' in args else b''))
''')
        binary = self.root / 'native.bin'
        binary.write_bytes(b'original')
        with self.assertRaisesRegex(runner.NativeArtifactError, 'round-trip digest/size mismatch'):
            runner.compress_native_artifact(binary, zstd)
        self.assertEqual(binary.read_bytes(), b'original')
        self.assertFalse((self.root / 'native.bin.zst').exists())

    def test_cache_gc_runs_in_build_workspace_and_retains_full_log(self):
        project_directory = self.root / 'files' / 'Example'
        project_directory.mkdir(parents=True)
        log = self.root / 'gc.log'
        with mock.patch.object(runner, 'execute', return_value={'exit_code': 0, 'timed_out': False,
                                                                 'wall_s': 0.1, 'log': str(log)}) as execute:
            result = runner.run_cache_gc(Path('/owned/kiln'), project_directory, log, 1, 8192)
        command, cwd, actual_log, timeout, runtime_jobs = execute.call_args.args
        self.assertEqual(Path(cwd), project_directory)
        self.assertEqual(Path(actual_log), log)
        self.assertEqual(result['project_context'], str(project_directory))
        self.assertEqual(command[-4:], ['/owned/kiln', '--one-shot', 'cache', 'gc'])
        self.assertEqual(timeout, 300)
        self.assertEqual(runtime_jobs, 1)
        self.assertEqual(result['exit_code'], 0)


class NodeVitestOracleTests(unittest.TestCase):
    def setUp(self):
        self.oracle = runner.NODE_VITEST_ORACLE
        self.mappings = runner.NODE_VITEST_MAPPINGS
        self.gaps = runner.NODE_VITEST_GAPS

    def compare_synthetic(self, candidate):
        runner.NODE_VITEST_ORACLE = {'files': {'X': {'source_sha256': 'source', 'cases': [
            {'id': 'X > case', 'mode': 'run', 'outcome': 'pass'}]}}}
        runner.NODE_VITEST_MAPPINGS = {'mappings': []}
        runner.NODE_VITEST_GAPS = {'cases': []}
        try:
            candidate.setdefault('complete', True)
            candidate.setdefault('timed_out', False)
            candidate.setdefault('matches_expected_exit', True)
            candidate.setdefault('summary', {'discovered': len(candidate.get('inventory', [])),
                                             'passed': sum(row.get('outcome') == 'pass' for row in candidate.get('cases', [])),
                                             'failed': sum(row.get('outcome') == 'fail' for row in candidate.get('cases', [])),
                                             'skipped': sum(row.get('outcome') == 'skip' for row in candidate.get('cases', []))})
            return runner.compare_node_vitest(candidate, 'X', 'source')
        finally:
            runner.NODE_VITEST_ORACLE = self.oracle
            runner.NODE_VITEST_MAPPINGS = self.mappings
            runner.NODE_VITEST_GAPS = self.gaps

    def test_unique_exact_id_is_compared_to_official_node_outcome(self):
        identifier = 'X > case'
        candidate = {'inventory': [{'id': identifier, 'mode': 'run'}],
                     'cases': [{'id': identifier, 'outcome': 'pass'}]}
        result = self.compare_synthetic(candidate)
        self.assertTrue(result['complete'])
        self.assertTrue(result['matches_modes'])
        self.assertTrue(result['matches_outcomes'])
        self.assertEqual(result['mapped_cases'], 1)

    def test_wrong_official_outcome_is_not_hidden_by_correct_mode(self):
        identifier = 'X > case'
        candidate = {'inventory': [{'id': identifier, 'mode': 'run'}],
                     'cases': [{'id': identifier, 'outcome': 'fail'}]}
        result = self.compare_synthetic(candidate)
        self.assertTrue(result['matches_modes'])
        self.assertFalse(result['matches_outcomes'])
        self.assertEqual(result['outcome_mismatches'][0]['expected'], 'pass')

    def test_duplicate_official_labels_stay_unmapped_without_explicit_evidence(self):
        oracle = copy.deepcopy(runner.NODE_VITEST_ORACLE)
        mappings = runner.NODE_VITEST_MAPPINGS
        gaps = runner.NODE_VITEST_GAPS
        runner.NODE_VITEST_ORACLE = {'files': {'X': {'source_sha256': 's', 'cases': [
            {'id': 'same', 'mode': 'run', 'outcome': 'pass'},
            {'id': 'same', 'mode': 'run', 'outcome': 'skip'}]}}}
        runner.NODE_VITEST_MAPPINGS = {'mappings': []}
        runner.NODE_VITEST_GAPS = {'cases': []}
        try:
            result = runner.compare_node_vitest(
                {'inventory': [{'id': 'same', 'mode': 'run'}],
                 'cases': [{'id': 'same', 'outcome': 'pass'}]}, 'X', 's')
        finally:
            runner.NODE_VITEST_ORACLE = oracle
            runner.NODE_VITEST_MAPPINGS = mappings
            runner.NODE_VITEST_GAPS = gaps
        self.assertFalse(result['complete'])
        self.assertIsNone(result['matches_outcomes'])
        self.assertEqual(result['unmapped_cases'][0]['official_candidates'], 2)

    def test_explicit_mapping_requires_source_anchor_and_is_one_to_one(self):
        oracle = copy.deepcopy(runner.NODE_VITEST_ORACLE)
        mappings = runner.NODE_VITEST_MAPPINGS
        gaps = runner.NODE_VITEST_GAPS
        runner.NODE_VITEST_ORACLE = {'files': {'X': {'source_sha256': 'source', 'cases': [
            {'id': 'official title', 'mode': 'run', 'outcome': 'pass'}]}}}
        runner.NODE_VITEST_MAPPINGS = {'mappings': [{
            'file': 'X', 'direct_id': 'adapter title', 'official_id': 'official title',
            'official_occurrence': 0, 'source_sha256': 'source', 'source_line': 12,
            'evidence': 'same pinned parameter row at source line 12'}]}
        runner.NODE_VITEST_GAPS = {'cases': []}
        try:
            candidate = {'inventory': [{'id': 'adapter title', 'mode': 'run'}],
                         'cases': [{'id': 'adapter title', 'outcome': 'pass'}]}
            candidate.update(complete=True, timed_out=False, matches_expected_exit=True,
                             summary={'discovered': 1, 'passed': 1, 'failed': 0, 'skipped': 0})
            result = runner.compare_node_vitest(candidate, 'X', 'source')
            runner.NODE_VITEST_MAPPINGS['mappings'][0]['source_sha256'] = 'stale'
            stale = runner.compare_node_vitest(
                candidate, 'X', 'source')
        finally:
            runner.NODE_VITEST_ORACLE = oracle
            runner.NODE_VITEST_MAPPINGS = mappings
            runner.NODE_VITEST_GAPS = gaps
        self.assertTrue(result['complete'])
        self.assertTrue(result['matches_outcomes'])
        self.assertFalse(stale['complete'])
        self.assertTrue(stale['mapping_errors'])

    def test_source_transform_gap_keeps_the_actual_mismatch_named(self):
        file = 'Formatter'
        identifier = 'Formatter > format > function'
        source_sha = self.oracle['files'][file]['source_sha256']
        candidate = {'inventory': [{'id': identifier, 'mode': 'run'}],
                     'cases': [{'id': identifier, 'outcome': 'fail'}]}
        result = runner.compare_node_vitest(candidate, file, source_sha)
        self.assertFalse(result['matches_outcomes'])
        self.assertEqual(result['outcome_mismatches'][0]['classification'],
                         'vitest_source_transform_difference')
        self.assertEqual(len(result['known_source_transform_gaps']), 1)

    def test_bun_harness_unsupported_remains_an_adapter_result(self):
        rows = runner.bun_adapter_unsupported([
            {'id': 'unavailable', 'outcome': 'fail', 'error': 'Error: HARNESS_UNSUPPORTED: vi.resetModules'},
            {'id': 'runtime-failure', 'outcome': 'fail', 'error': 'TypeError: bad'}])
        self.assertEqual([row['id'] for row in rows], ['unavailable'])

    def test_candidate_envelope_requires_complete_summary_and_selected_inventory(self):
        candidate = {'inventory': [{'id': 'X > case', 'mode': 'run'}],
                     'cases': [{'id': 'X > case', 'outcome': 'pass'}],
                     'complete': False, 'timed_out': False, 'matches_expected_exit': True,
                     'summary': {'discovered': 1, 'passed': 1, 'failed': 0, 'skipped': 0}}
        result = self.compare_synthetic(candidate)
        self.assertFalse(result['complete'])
        self.assertIn('runtime_envelope_incomplete', result['envelope_errors'])

    def test_stale_archive_summary_is_rejected_from_derived_case_totals(self):
        oracle = runner.NODE_VITEST_ORACLE
        runner.NODE_VITEST_ORACLE = copy.deepcopy(oracle)
        runner.NODE_VITEST_ORACLE['summary']['pass'] += 1
        try:
            with self.assertRaisesRegex(ValueError, 'per-file totals|archive summary'):
                runner.validate_node_vitest_policy()
        finally:
            runner.NODE_VITEST_ORACLE = oracle

    def test_changed_archive_case_with_stale_totals_is_rejected(self):
        oracle = runner.NODE_VITEST_ORACLE
        runner.NODE_VITEST_ORACLE = copy.deepcopy(oracle)
        first_file = next(iter(runner.NODE_VITEST_ORACLE['files'].values()))
        first_file['cases'].pop()
        try:
            with self.assertRaisesRegex(ValueError, 'per-file totals|archive summary'):
                runner.validate_node_vitest_policy()
        finally:
            runner.NODE_VITEST_ORACLE = oracle

    def test_parameterized_mapping_requires_argument_value_and_source_row(self):
        mappings = runner.NODE_VITEST_MAPPINGS
        runner.NODE_VITEST_MAPPINGS = copy.deepcopy(mappings)
        try:
            row = next(x for x in runner.NODE_VITEST_MAPPINGS['mappings']
                       if x.get('parameter_index') is not None)
            del row['parameter_value']
            with self.assertRaisesRegex(ValueError, 'parameterized mapping lacks exact source argument'):
                runner.validate_node_vitest_policy()
        finally:
            runner.NODE_VITEST_MAPPINGS = mappings

    def test_date_time_parameter_values_map_one_to_one_to_official_titles(self):
        rows = [x for x in self.mappings['mappings'] if x['file'] == 'DateTime']
        source_sha = self.oracle['files']['DateTime']['source_sha256']
        mapped = runner.node_vitest_case_map(
            'DateTime', [{'id': row['direct_id'], 'mode': 'run'} for row in rows], source_sha)
        self.assertEqual(len(rows), 39)
        self.assertEqual(len(mapped['mapped']), 39)
        self.assertEqual(mapped['mapping_errors'], [])
        self.assertEqual(len({row['official_index'] for row in mapped['mapped']}), 39)

    def test_full_official_inventory_is_covered_by_exact_and_explicit_source_maps(self):
        explicit_by_file = {}
        for row in self.mappings['mappings']:
            explicit_by_file.setdefault(row['file'], []).append(row)
        mapped_total = 0
        for file, official_file in self.oracle['files'].items():
            occurrences = {}
            for case in official_file['cases']:
                occurrences[case['id']] = occurrences.get(case['id'], 0) + 1
            explicit = explicit_by_file.get(file, [])
            overridden = {row['official_id'] for row in explicit}
            direct_ids = [identifier for identifier, count in occurrences.items()
                          if count == 1 and identifier not in overridden]
            direct_ids.extend(row['direct_id'] for row in explicit)
            mapping = runner.node_vitest_case_map(
                file, [{'id': identifier, 'mode': 'run'} for identifier in direct_ids],
                official_file['source_sha256'])
            self.assertTrue(mapping['complete'], (file, mapping))
            self.assertEqual(len(mapping['mapped']), len(official_file['cases']))
            mapped_total += len(mapping['mapped'])
        self.assertEqual(mapped_total, self.oracle['summary']['registered'])

    def test_comparison_verdicts_keep_bun_divergence_and_node_coverage_separate(self):
        report = {'selected_files': ['X'], 'files': [{'file': 'X', 'engines': {
            'bun': {'bun_adapter_unsupported_cases': [{'id': 'adapter-gap'}]},
            'native': {'complete': True, 'node_vitest': {'complete': False, 'mapped_cases': 1,
                                       'unmapped_cases': [{'id': 'unmapped'}],
                                       'official_unmapped_cases': [{'id': 'official'}],
                                       'mode_mismatches': [], 'outcome_mismatches': []},
                       'matches_reference_inventory': True,
                       'matches_reference_modes': False,
                       'matches_reference_outcomes': True}}}]}
        result = runner.comparison_verdicts(report, ['bun', 'native'])
        self.assertEqual(result['node_vitest_conformance']['native']['status'], 'incomplete')
        self.assertEqual(result['node_vitest_conformance']['native']['unmapped_direct_cases'], 1)
        self.assertEqual(result['direct_bun_differential']['status'], 'diverged')
        self.assertEqual(result['direct_bun_differential']['mode_mismatches'], 1)
        self.assertEqual(result['bun_adapter_unsupported'][0]['id'], 'adapter-gap')

    def test_partial_selected_file_report_is_not_native_conformance(self):
        complete = {'complete': True, 'node_vitest': {'complete': True, 'mapped_cases': 1,
                    'unmapped_cases': [], 'official_unmapped_cases': [], 'mode_mismatches': [],
                    'outcome_mismatches': []}}
        report = {'selected_files': ['A', 'B'],
                  'files': [{'file': 'A', 'engines': {'native': complete}}]}
        result = runner.comparison_verdicts(report, ['native'])
        native = result['node_vitest_conformance']['native']
        self.assertEqual(native['status'], 'incomplete')
        self.assertIn('selected_files_missing_from_report', native['coverage_errors'])
        self.assertEqual(native['runtime_incomplete_files'], [])

    def test_selected_engine_with_no_runtime_records_is_incomplete(self):
        report = {'selected_files': ['A'], 'files': []}
        result = runner.comparison_verdicts(report, ['native'])
        native = result['node_vitest_conformance']['native']
        self.assertEqual(native['status'], 'incomplete')
        self.assertEqual(native['coverage_errors'], ['runtime_records_missing'])

    def test_node_conformance_pass_is_independent_of_bun_differential(self):
        native = {'complete': True, 'matches_reference_inventory': True,
                  'matches_reference_modes': False, 'matches_reference_outcomes': False,
                  'node_vitest': {'complete': True, 'mapped_cases': 1, 'unmapped_cases': [],
                                  'official_unmapped_cases': [], 'mode_mismatches': [],
                                  'outcome_mismatches': []}}
        bun = {'complete': True, 'matches_reference_inventory': True,
               'matches_reference_modes': True, 'matches_reference_outcomes': True}
        report = {'selected_files': ['A'], 'files': [{'file': 'A', 'engines': {'native': native, 'bun': bun}}]}
        result = runner.comparison_verdicts(report, ['native', 'bun'])
        self.assertEqual(result['node_vitest_conformance']['native']['status'], 'pass')
        self.assertEqual(result['direct_bun_differential']['status'], 'diverged')


if __name__ == '__main__':
    unittest.main()
