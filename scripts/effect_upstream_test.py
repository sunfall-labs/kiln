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

    def test_undeclared_mode_difference_stays_a_mismatch(self):
        other_id = 'Cache > unrelated case'
        reference = {'inventory': [{'id': other_id, 'mode': 'skip'}],
                     'cases': [{'id': other_id, 'outcome': 'skip'}]}
        candidate = {'inventory': [{'id': other_id, 'mode': 'run'}],
                     'cases': [{'id': other_id, 'outcome': 'pass'}]}
        result = runner.compare_reference(reference, candidate, 'native', 'Cache')
        self.assertFalse(result['modes'])
        self.assertFalse(result['outcomes'])


if __name__ == '__main__':
    unittest.main()
