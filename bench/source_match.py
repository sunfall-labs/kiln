"""Hash and compare tracked source trees for matched performance controls."""

from __future__ import annotations

import hashlib
import os
import stat
import subprocess
from pathlib import Path


def git_head(path: Path) -> str:
    result = subprocess.run(["git", "-C", str(path), "rev-parse", "HEAD"],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode:
        raise RuntimeError(f"cannot read source revision for {path}")
    return result.stdout.decode().strip()


def tracked_paths(root: Path) -> list[str]:
    result = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--cached"],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode:
        raise RuntimeError(f"cannot list tracked files under {root}: "
                           f"{result.stderr.decode('utf-8', 'replace').strip()}")
    return sorted(os.fsdecode(path) for path in result.stdout.split(b"\0") if path)


def worktree_entry(root: Path, relative: str) -> tuple[str, int, bytes]:
    """Read indexed path content from disk, including dirty tracked overlays."""
    path = root / relative
    try:
        info = path.lstat()
    except OSError as error:
        raise RuntimeError(f"tracked source file is missing under {root}: {relative}: {error}") from error
    mode = 0o111 if info.st_mode & 0o111 else 0
    if stat.S_ISLNK(info.st_mode):
        return "symlink", mode, os.fsencode(os.readlink(path))
    if stat.S_ISREG(info.st_mode):
        return "file", mode, path.read_bytes()
    if stat.S_ISDIR(info.st_mode):
        return "submodule", mode, git_head(path).encode("ascii")
    raise RuntimeError(f"unsupported tracked file kind under {root}: {relative}")


def digest_entry(digest: "hashlib._Hash", relative: str,
                 entry: tuple[str, int, bytes]) -> None:
    kind, mode, contents = entry
    name = os.fsencode(relative)
    digest.update(len(name).to_bytes(8, "big"))
    digest.update(name)
    digest.update(kind.encode("ascii") + b"\0")
    digest.update(mode.to_bytes(4, "big"))
    digest.update(len(contents).to_bytes(8, "big"))
    digest.update(contents)


def compare_sources(production: Path, control: Path, *, allowed_differences: set[str],
                    required_equal: dict[str, Path]) -> dict[str, object]:
    """Require exact tracked-tree match except named files, and match harness inputs."""
    production, control = production.resolve(), control.resolve()
    if not production.is_dir() or not control.is_dir():
        raise RuntimeError("production and control source paths must be directories")
    prod_paths, control_paths = tracked_paths(production), tracked_paths(control)
    if prod_paths != control_paths:
        raise RuntimeError("tracked source inventories differ: "
                           f"production-only={sorted(set(prod_paths)-set(control_paths))[:8]!r}, "
                           f"control-only={sorted(set(control_paths)-set(prod_paths))[:8]!r}")
    if not allowed_differences.issubset(prod_paths):
        raise RuntimeError(f"allowed tracked source paths missing: {sorted(allowed_differences-set(prod_paths))}")
    inventory, common, prod_tree, control_tree = (hashlib.sha256() for _ in range(4))
    diffs: dict[str, dict[str, str]] = {}
    for relative in prod_paths:
        left, right = worktree_entry(production, relative), worktree_entry(control, relative)
        name = os.fsencode(relative)
        inventory.update(len(name).to_bytes(8, "big")); inventory.update(name)
        digest_entry(prod_tree, relative, left); digest_entry(control_tree, relative, right)
        if left != right:
            if relative not in allowed_differences:
                raise RuntimeError(f"tracked source differs outside allowed files: {relative}")
            diffs[relative] = {"production_sha256": hashlib.sha256(left[2]).hexdigest(),
                               "control_sha256": hashlib.sha256(right[2]).hexdigest()}
        else:
            digest_entry(common, relative, left)
    if set(diffs) != allowed_differences:
        raise RuntimeError(f"expected differences {sorted(allowed_differences)}, found {sorted(diffs)}")
    harness_inputs = {}
    for relative, harness_path in required_equal.items():
        try:
            expected = harness_path.read_bytes()
            left, right = ((root / relative).read_bytes() for root in (production, control))
        except OSError as error:
            raise RuntimeError(f"cannot read required workload/lock {relative}: {error}") from error
        if left != expected or right != expected:
            raise RuntimeError(f"{relative} must match harness corpus and both source trees")
        harness_inputs[relative] = hashlib.sha256(expected).hexdigest()
    return {"production_head": git_head(production), "control_head": git_head(control),
            "tracked_file_count": len(prod_paths), "tracked_path_inventory_sha256": inventory.hexdigest(),
            "matched_source_sha256": common.hexdigest(),
            "production_tracked_tree_sha256": prod_tree.hexdigest(),
            "control_tracked_tree_sha256": control_tree.hexdigest(),
            "allowed_source_differences": diffs, "harness_inputs_sha256": harness_inputs}
