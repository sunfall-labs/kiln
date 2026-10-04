import importlib.util
import copy
from pathlib import Path
import unittest

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
            result = runner.compare_node_vitest(
                {'inventory': [{'id': 'adapter title', 'mode': 'run'}],
                 'cases': [{'id': 'adapter title', 'outcome': 'pass'}]}, 'X', 'source')
            runner.NODE_VITEST_MAPPINGS['mappings'][0]['source_sha256'] = 'stale'
            stale = runner.compare_node_vitest(
                {'inventory': [{'id': 'adapter title', 'mode': 'run'}],
                 'cases': [{'id': 'adapter title', 'outcome': 'pass'}]}, 'X', 'source')
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

    def test_comparison_verdicts_keep_bun_divergence_and_node_coverage_separate(self):
        report = {'files': [{'file': 'X', 'engines': {
            'bun': {'bun_adapter_unsupported_cases': [{'id': 'adapter-gap'}]},
            'native': {'node_vitest': {'complete': False, 'mapped_cases': 1,
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


if __name__ == '__main__':
    unittest.main()
