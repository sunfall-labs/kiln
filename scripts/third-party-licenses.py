#!/usr/bin/env python3
"""Write the license notices for every crate linked into a release.

    scripts/third-party-licenses.py OUTPUT_FILE

A release binary links its dependencies statically, and the MIT, BSD and Apache licenses they
carry each require their text to travel with copies of the software. This collects the license
files of every crate in the dependency graph of the shipped binaries (`kiln-cli` and the native
provider archives), from wherever Cargo unpacked them, and refuses when a crate has neither a
license file nor an SPDX expression it can name -- a release must not ship with a notice missing.
"""
import json
import os
import subprocess
import sys

ROOTS = ("kiln-cli", "kiln-native-provider-ffi")
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unicode")


def metadata():
    output = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(output)


def reachable(data):
    packages = {package["id"]: package for package in data["packages"]}
    nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
    workspace = set(data["workspace_members"])
    start = [pid for pid in workspace if packages[pid]["name"] in ROOTS]
    if len(start) != len(ROOTS):
        sys.exit(f"third-party-licenses: could not find all of {ROOTS} in the workspace")
    seen, stack = set(), list(start)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            # Build scripts and proc macros run at compile time and are not linked into the
            # binary, but their kinds are per-edge; keep only normal edges.
            if any(kind.get("kind") in (None, "normal") for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return [packages[pid] for pid in sorted(seen) if pid not in workspace]


def license_files(package):
    directory = os.path.dirname(package["manifest_path"])
    found = []
    if package.get("license_file"):
        found.append(os.path.join(directory, package["license_file"]))
    for name in sorted(os.listdir(directory)):
        lowered = name.lower()
        if lowered.startswith(LICENSE_PREFIXES) and os.path.isfile(os.path.join(directory, name)):
            path = os.path.join(directory, name)
            if path not in found:
                found.append(path)
    return found


# Declarations embedded in the binary that are not crates: (name, source, directory holding LICENSE).
EMBEDDED = (
    ("@types/node (embedded by `kiln check`)", "https://github.com/DefinitelyTyped/DefinitelyTyped/tree/master/types/node",
     "crates/compile/kiln-type-checker/lib/vendor/types-node"),
    ("undici-types (embedded by `kiln check`)", "https://github.com/nodejs/undici",
     "crates/compile/kiln-type-checker/lib/vendor/undici-types"),
)


def embedded_sections():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    sections = []
    for name, source, directory in EMBEDDED:
        path = os.path.join(root, directory, "LICENSE")
        if not os.path.isfile(path):
            sys.exit(f"third-party-licenses: {path} is missing")
        with open(path, errors="replace") as handle:
            text = handle.read().rstrip()
        sections.append(f"## {name}\n\nLicense: MIT\nSource: {source}\n\n### LICENSE\n\n{text}\n")
    return sections


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: third-party-licenses.py OUTPUT_FILE")
    packages = reachable(metadata())
    missing = []
    sections = []
    for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
        files = license_files(package)
        expression = package.get("license") or ""
        if not files and not expression:
            missing.append(f'{package["name"]} {package["version"]}')
            continue
        body = [f'## {package["name"]} {package["version"]}', ""]
        if expression:
            body.append(f"License: {expression}")
        if package.get("repository"):
            body.append(f'Source: {package["repository"]}')
        body.append("")
        if files:
            for path in files:
                with open(path, errors="replace") as handle:
                    body.append(f"### {os.path.basename(path)}")
                    body.append("")
                    body.append(handle.read().rstrip())
                    body.append("")
        else:
            body.append(
                f"No license file is included in the published crate; it is distributed under "
                f"{expression}, whose standard text applies."
            )
            body.append("")
        sections.append("\n".join(body))
    if missing:
        sys.exit("third-party-licenses: no license information for: " + ", ".join(missing))
    header = (
        "# Third-party software in kiln\n\n"
        f"kiln links {len(packages)} third-party crates into its release binaries. Their licenses "
        "follow, one section per crate, generated by `scripts/third-party-licenses.py`.\n\n"
    )
    with open(sys.argv[1], "w") as handle:
        handle.write(header + "\n".join(sections + embedded_sections()))
    print(f"{len(packages)} crates", file=sys.stderr)


if __name__ == "__main__":
    main()
