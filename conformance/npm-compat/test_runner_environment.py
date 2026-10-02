"""Check the oracle's actual child environment against native profile policy."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('npm_runner', Path(__file__).with_name('runner.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ProfileEnvironmentTests(unittest.TestCase):
    def child_mode(self, profile, inherited_mode):
        inherited = dict(os.environ)
        if inherited_mode is None:
            inherited.pop('NODE_ENV', None)
        else:
            inherited['NODE_ENV'] = inherited_mode
        environment = runner.profile_environment(profile, inherited)
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / 'child.log'
            result = runner.command(['node', '-e', 'console.log(process.env.NODE_ENV)'],
                                    prefix, 10, environment)
            self.assertEqual(result['exit'], 0)
            output = prefix.with_suffix('.stdout').read_text().strip()
        self.assertEqual(inherited.get('NODE_ENV'), inherited_mode)
        return output

    def test_release_oracle_uses_native_production_mode(self):
        self.assertEqual(self.child_mode('native-release', 'development'), 'production')

    def test_development_oracle_preserves_explicit_mode(self):
        self.assertEqual(self.child_mode('native-dev', 'test'), 'test')

    def test_development_oracle_preserves_unset_mode(self):
        self.assertEqual(self.child_mode('native-dev', None), 'undefined')


if __name__ == '__main__':
    unittest.main()
