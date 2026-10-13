#!/usr/bin/env python3
"""Create npm platform packages from checked, complete release archives; no builds/downloads."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import tarfile

TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux", "x64", "glibc"),
    "aarch64-apple-darwin": ("darwin", "arm64", None),
}
ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("archives", type=Path, nargs="+")
    args = parser.parse_args()
    template = json.loads((ROOT / "packages/kiln-cli/package.json").read_text())
    version = template["version"]
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("output directory must be empty (preserve previously staged packages)")
    output.mkdir(parents=True, exist_ok=True)
    # Reserve the pending Mac dependency now: the immutable launcher version must also work
    # when its matching platform package is published later. Missing optional packages are safe.
    dependencies = {f"{template['name']}-{os_name}-{cpu}": version
                    for os_name, cpu, _ in TARGETS.values()}
    staged = set()
    for archive in args.archives:
        target = next((t for t in TARGETS if archive.name == f"kiln-{version}-{t}.tar.gz"), None)
        if target is None:
            parser.error(f"unsupported target or version: {archive.name}; expected {version}")
        os_name, cpu, libc = TARGETS[target]
        slug = f"kiln-{os_name}-{cpu}"
        name = f"{template['name']}-{os_name}-{cpu}"
        if name in staged:
            parser.error(f"duplicate target: {target}")
        checksum = Path(str(archive) + ".sha256")
        expected = checksum.read_text().split()[0]
        digest = hashlib.sha256()
        with archive.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != expected:
            parser.error(f"checksum mismatch: {archive}")
        package = output / slug
        package.mkdir()
        prefix = f"kiln-{version}-{target}"
        with tarfile.open(archive, "r:gz") as tar:
            for entry in tar:
                relative = PurePosixPath(entry.name)
                if not relative.parts or relative.is_absolute() or ".." in relative.parts or relative.parts[0] != prefix:
                    parser.error(f"unsafe archive path: {entry.name}")
                if not (entry.isdir() or entry.isfile()):
                    parser.error(f"unsupported archive entry: {entry.name}")
                dest = package.joinpath(*relative.parts[1:])
                if entry.isdir():
                    dest.mkdir(parents=True, exist_ok=True)
                else:
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    with tar.extractfile(entry) as src, dest.open("wb") as dst:
                        shutil.copyfileobj(src, dst)
                    dest.chmod(entry.mode & 0o777)
        for required in ["bin/kiln", "LICENSE", "SECURITY.md", "THIRD-PARTY-LICENSES.md"]:
            if not (package / required).is_file():
                parser.error(f"archive is incomplete: missing {required}")
        if not list((package / "lib/kiln").glob("libkiln_native_provider_ffi*.a")):
            parser.error("archive has no native provider libraries")
        manifest = {
            "name": name, "version": version, "description": f"kiln native runtime for {os_name} {cpu}",
            "license": "MIT", "repository": template["repository"], "os": [os_name], "cpu": [cpu],
            "files": ["bin", "lib", "LICENSE", "README.md", "SECURITY.md", "THIRD-PARTY-LICENSES.md"],
            "publishConfig": {"access": "public"},
        }
        if libc:
            manifest["libc"] = [libc]
        (package / "package.json").write_text(json.dumps(manifest, indent=2) + "\n")
        staged.add(name)
        print(f"staged {name}@{version}: {digest.hexdigest()}")
    launcher = output / "kiln"
    shutil.copytree(ROOT / "packages/kiln-cli", launcher)
    shutil.copy2(ROOT / "LICENSE", launcher / "LICENSE")
    template["optionalDependencies"] = dependencies
    (launcher / "package.json").write_text(json.dumps(template, indent=2) + "\n")
    (launcher / "bin/kiln.cjs").chmod(0o755)
    print(f"staged {template['name']}@{version}; publish platform packages first")


if __name__ == "__main__":
    main()
