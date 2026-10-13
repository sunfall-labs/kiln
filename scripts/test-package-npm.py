#!/usr/bin/env python3
"""Verify archive integrity and complete provider-layout preservation for npm distribution."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
VERSION = json.loads((ROOT / "packages/kiln-cli/package.json").read_text())["version"]


class PackageTests(unittest.TestCase):
    def stage(self, base, extra=None, corrupt=False):
        prefix = f"kiln-{VERSION}-x86_64-unknown-linux-gnu"
        archive = base / f"{prefix}.tar.gz"
        payloads = {
            "bin/kiln": b"#!/bin/sh\nexit 0\n", "LICENSE": b"MIT", "SECURITY.md": b"authority",
            "README.md": b"runtime", "THIRD-PARTY-LICENSES.md": b"licences",
            "lib/kiln/libkiln_native_provider_ffi_common.a": b"common",
            "lib/kiln/libkiln_native_provider_ffi_full.a": b"full",
            "lib/kiln/common.json": b'{"variant":"common"}',
            "lib/kiln/full.json": b'{"variant":"full"}',
        }
        with tarfile.open(archive, "w:gz") as tar:
            for name, body in payloads.items():
                member = tarfile.TarInfo(f"{prefix}/{name}")
                member.size = len(body)
                member.mode = 0o755 if name == "bin/kiln" else 0o644
                tar.addfile(member, io.BytesIO(body))
            if extra:
                tar.addfile(tarfile.TarInfo(extra), io.BytesIO(b""))
        digest = "0" * 64 if corrupt else hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(str(archive) + ".sha256").write_text(f"{digest}  {archive.name}\n")
        output = base / "npm"
        result = subprocess.run(["python3", str(ROOT / "scripts/package-npm.py"), "--output",
                                 str(output), str(archive)], capture_output=True, text=True)
        return result, output, payloads

    def test_preserves_all_provider_bytes_and_exact_dependency_version(self):
        with tempfile.TemporaryDirectory() as directory:
            result, output, payloads = self.stage(Path(directory))
            self.assertEqual(result.returncode, 0, result.stderr)
            platform = output / "kiln-linux-x64"
            for name, body in payloads.items():
                self.assertEqual((platform / name).read_bytes(), body)
            launcher = json.loads((output / "kiln/package.json").read_text())
            self.assertEqual(launcher["optionalDependencies"], {
                f'{launcher["name"]}-linux-x64': VERSION,
                f'{launcher["name"]}-darwin-arm64': VERSION,
            })
            self.assertNotIn("scripts", launcher)
            binary = json.loads((platform / "package.json").read_text())
            self.assertEqual(binary["libc"], ["glibc"])
            self.assertTrue((platform / "bin/kiln").stat().st_mode & 0o111)

    def test_rejects_corrupt_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            result, _, _ = self.stage(Path(directory), corrupt=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("checksum mismatch", result.stderr)

    def test_rejects_path_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            result, _, _ = self.stage(Path(directory), extra="../escape")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unsafe archive path", result.stderr)
            self.assertFalse((Path(directory) / "escape").exists())


if __name__ == "__main__":
    unittest.main()
