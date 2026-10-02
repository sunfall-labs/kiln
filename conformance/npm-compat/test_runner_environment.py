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

    def test_every_profile_uses_utc_even_when_parent_timezone_is_local(self):
        inherited = dict(os.environ, TZ='America/Denver')
        for profile in ('native-dev', 'native-release'):
            environment = runner.profile_environment(profile, inherited)
            self.assertEqual(environment['TZ'], 'UTC')
            with tempfile.TemporaryDirectory() as directory:
                prefix = Path(directory) / 'zone.log'
                result = runner.command(
                    ['node', '-e', 'console.log(Intl.DateTimeFormat().resolvedOptions().timeZone)'],
                    prefix, 10, environment)
                self.assertEqual(result['exit'], 0)
                self.assertEqual(prefix.with_suffix('.stdout').read_text().strip(), 'UTC')
        self.assertEqual(inherited['TZ'], 'America/Denver')


if __name__ == '__main__':
    unittest.main()
