# 12. User documentation

Status: resolved

A page for users: what the parallel built-ins are, the guarantee (identical results to the
sequential method, never a Node difference), how to read `--explain-parallel`, what each tier
allows, how the pool is sized, and how to remove the extensions (`KILN_EXTENSIONS=0`,
`--no-kiln-extensions`). Update it as each tier lands.

## Acceptance

- The page exists in `docs/` and the README links to it.

## Comments

2026-09-27: `docs/parallel-builtins.md` documents the methods, their sequential
semantics, the fixed reduce tree, proof tiers, report format, current pool budget,
and both extension switches. README links to it.
