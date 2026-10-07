# `Array.prototype.fill` Test262 subtree

The frozen combined interpreter CLI was tested against the pinned Test262 subtree. This is a
subtree result, not a whole-corpus conformance claim.

## Inputs and command

- Candidate source commit: `96aaf9399a16e19e40be026e4cb0b597ca47c0f2`
- Frozen CLI: `~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/effect-runtime-followup/.scratch/runtime-validation/kiln-0320d657acbe69fba302a30d509f6540b4b58e9187b9da9cda9d8b6dc8ce474a`
- CLI SHA-256: `0320d657acbe69fba302a30d509f6540b4b58e9187b9da9cda9d8b6dc8ce474a`
- Pinned Test262 checkout: `~/.cache/kiln/test262-771005236e`
- Test262 commit: `771005236e88a909635104e03ba12559688c0172`
- Selected subtree: `built-ins/Array/prototype/fill`
- Runner: `scripts/test262.sh` SHA-256 `f5678a20c303530dcb8ec3968b5dd9de0660aa95bf42e4d9e7fa4d847160e499`
- Corpus runner: `scripts/test262_corpus.py` SHA-256 `5c2a2c29605d0342e212b5a7eb299c67eae2f8cd276f07e2fea00bafc8bcd018`

The runner used the unchanged 4 GiB memory cap, one job and one Rayon thread for both candidate
and baseline:

```sh
PATH=~/.local/share/mise/installs/rust/1.97.1:~/.local/share/mise/installs/mr-boxington/1.22.0/.mise-bins:/usr/bin:/bin \
RUSTUP_TOOLCHAIN=1.97.1 \
KILN_TEST262_KILN=~/Work/kiln/.scratch/effect-sandbox-integration/.scratch/effect-runtime-followup/.scratch/runtime-validation/kiln-0320d657acbe69fba302a30d509f6540b4b58e9187b9da9cda9d8b6dc8ce474a \
KILN_TEST262_MEMORY_MB=4096 KILN_JOBS=1 RAYON_NUM_THREADS=1 \
scripts/test262.sh --out <unique-output-path> --path built-ins/Array/prototype/fill
```

The concrete output directory was
`~/Work/kiln/.scratch/effect-arrayfill-dense-kernel/.scratch/arrayfill-validation/test262-fill-20261007`.

## Result

The inventory contained 22 files / 44 strictness variants: 40 passed and 4 failed, with no
unsupported or skipped variants. Repeating the same subtree against the prior frozen CLI
`c61bb82a8afdd67f718bb3c3a2be7fe92b44b2c37d82b323c01b6ac82eabb948` produced the identical
40/4 result and the identical four failing case IDs. Both variants of `resizable-buffer.js` and
`typed-array-resize.js` fail in both binaries. Their tests call `Array.prototype.fill` on
TypedArrays backed by resizable buffers; the new fast path is restricted to `Value::Array` and is
not selected by those calls. The per-case comparison is in `comparison.json`; full records are in
`test262.log`, `results.json`, `baseline-c61-test262.log`, and `baseline-c61-results.json`.

The separate new differential fixture `array-fill-dense-range.js` was reported by the root runner
to match Node 26.8.1 and Bun 1.4.2 (one matched, zero refused, zero diverged). That result came from
the root runner, not from this Test262 invocation.

Digests:

```text
results.json  ee34ec29a5ff8b70b750695b5b2bb6d933197a6747e3df646dbdf4e3b751f820
test262.log   73f23da593bbb8df2035fd2a1ce2e2ba2588fed0052469dc38aae0d8e4b37f90
baseline-c61-results.json  a639c460ce3e3eb518e086c6d8c9f68b41d7607ea5d14822ea61b339392d9194
baseline-c61-test262.log  4de8fa4f185933c0a767707c1266240b64975d8b72789fc790f8ebbf42e35a06
comparison.json  1774d9cba3b58bd54d98e0a1ef2b34df2b3143376194bcadc7bfc9cacfeaf25f
```
