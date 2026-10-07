#!/usr/bin/env python3
"""Controls for the immutable native inputs used by long Effect runs."""
import hashlib
import importlib.util
import json
import os
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('effect_upstream', Path(__file__).with_name('effect-upstream.py'))
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)


class ProviderSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        self.toolchain = self.root / 'toolchain'
        self.toolchain.mkdir()
        self.archive = self.source / 'provider.a'
        self.archive.write_bytes(b'old provider')
        self.manifest = self.source / 'kiln-native-provider.json'
        self.write_manifest()

    def write_manifest(self, archive='provider.a', digest=None):
        self.manifest.write_text(json.dumps({'archive': archive,
            'archive_sha256': digest or hashlib.sha256(self.archive.read_bytes()).hexdigest(),
            'source_sha256': 'source identity', 'provider_abi': 82}))

    def test_source_edits_do_not_change_snapshot(self):
        result = runner.snapshot_provider(self.manifest, self.toolchain)
        self.archive.write_bytes(b'new provider')
        self.write_manifest()
        snapshot = Path(result['manifest'])
        self.assertEqual((snapshot.parent / 'provider.a').read_bytes(), b'old provider')
        self.assertEqual(json.loads(snapshot.read_text())['archive_sha256'], result['archive_sha256'])

    def test_distinct_provider_versions_have_distinct_owned_paths(self):
        first = runner.snapshot_provider(self.manifest, self.toolchain)
        self.archive.write_bytes(b'new provider')
        self.write_manifest()
        second = runner.snapshot_provider(self.manifest, self.toolchain)
        self.assertNotEqual(first['manifest'], second['manifest'])
        self.assertEqual((Path(first['manifest']).parent / 'provider.a').read_bytes(), b'old provider')

    def test_manifest_mismatch_fails_before_publication(self):
        self.write_manifest(digest='0' * 64)
        with self.assertRaisesRegex(RuntimeError, 'does not match'):
            runner.snapshot_provider(self.manifest, self.toolchain)
        self.assertEqual(list(self.toolchain.iterdir()), [])

    def test_corrupt_existing_snapshot_is_rejected(self):
        result = runner.snapshot_provider(self.manifest, self.toolchain)
        (Path(result['manifest']).parent / 'provider.a').write_bytes(b'corrupt')
        with self.assertRaisesRegex(RuntimeError, 'corrupt provider snapshot'):
            runner.snapshot_provider(self.manifest, self.toolchain)

    def test_paths_cannot_escape_the_package(self):
        for name in ('../provider.a', '/provider.a', 'sub/provider.a', '..', '.', '', r'..\provider.a'):
            with self.subTest(name=name):
                self.write_manifest(archive=name)
                with self.assertRaisesRegex(RuntimeError, 'one file name'):
                    runner.snapshot_provider(self.manifest, self.toolchain)

    def test_changed_input_during_copy_is_rejected_and_staging_removed(self):
        def changed_copy(source, destination):
            Path(destination).write_bytes(b'changed provider')
        with patch.object(runner.shutil, 'copy2', changed_copy):
            with self.assertRaisesRegex(RuntimeError, 'changed while'):
                runner.snapshot_provider(self.manifest, self.toolchain)
        self.assertEqual(list(self.toolchain.iterdir()), [])

    def test_execution_inherits_snapshot_environment_into_children(self):
        result = runner.snapshot_provider(self.manifest, self.toolchain)
        log = self.root / 'execution.log'
        child = 'import os; print(os.environ["KILN_NATIVE_PROVIDER_MANIFEST"])'
        parent = 'import subprocess,sys; subprocess.run([sys.executable,"-c",' + repr(child) + '],check=True)'
        execution = runner.execute([sys.executable, '-c', parent], self.root, log, 5, 1,
                                   {'KILN_NATIVE_PROVIDER_MANIFEST': result['manifest']})
        self.assertEqual(execution['exit_code'], 0)
        self.assertEqual(log.read_text().strip(), result['manifest'])

    def test_explicit_provider_does_not_build_source_fallback(self):
        out = self.root / 'out'
        (out / 'toolchain').mkdir(parents=True)
        with patch.object(runner, 'execute', side_effect=AssertionError('must use supplied provider')):
            result = runner.prepare_provider(self.manifest, self.root / 'kiln', out, 1, 2, 8192)
        self.assertIsNone(result['preparation'])
        self.assertEqual(result['archive_sha256'], hashlib.sha256(self.archive.read_bytes()).hexdigest())

    def test_default_preparation_uses_pinned_toolchain_and_records_build(self):
        out = self.root / 'out'
        (out / 'toolchain').mkdir(parents=True)
        def package(command, cwd, log, timeout, jobs):
            self.assertIn('mise', command)
            self.assertIn('exec', command)
            self.assertEqual(command[3], '8192')
            self.assertEqual(jobs, 2)
            self.assertGreaterEqual(timeout, 300)
            package = Path(command[-1])
            package.mkdir()
            (package / 'provider.a').write_bytes(self.archive.read_bytes())
            (package / 'kiln-native-provider.json').write_bytes(self.manifest.read_bytes())
            return {'exit_code': 0, 'log': str(log)}
        with patch.dict(os.environ, {}, clear=True), patch.object(runner, 'execute', package):
            result = runner.prepare_provider(None, self.root / 'kiln', out, 1, 2, 8192)
        self.assertEqual(result['preparation']['exit_code'], 0)
        self.assertTrue(Path(result['manifest']).is_file())

    def test_valid_existing_snapshot_is_reused(self):
        first = runner.snapshot_provider(self.manifest, self.toolchain)
        with patch.object(runner.shutil, 'copy2', side_effect=AssertionError('must reuse snapshot')):
            self.assertEqual(runner.snapshot_provider(self.manifest, self.toolchain), first)


if __name__ == '__main__':
    unittest.main()
