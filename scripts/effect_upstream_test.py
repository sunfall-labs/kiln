import importlib.util
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


if __name__ == '__main__':
    unittest.main()
