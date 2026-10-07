# Native arguments-length probe scope audit

This change keeps the parity corpus focused on behaviors the current engines both support. The
ordinary MJS probe retains argc-only calls (including surplus actuals), object-observation and
arrow/eval controls, allocation, async behavior, new.target, and thrown-error behavior. It adds a
formal object argument that remains live across object/array allocation and is read afterward,
with two extra actual arguments. Node 26.8.1 and Bun 1.4.2 agree on the updated MJS oracle in
`raw/node-supported-mjs.log` and `raw/bun-supported-mjs.log`.

The previous MJS also tested a generator's `arguments.length` and sloppy parameter/arguments
aliasing in dynamically created `Function` bodies. The standalone CJS probe tested the same
sloppy aliasing in statically compiled CommonJS functions. Those cases are preserved under
`reproductions/` with the full original MJS and raw Node, Bun, VM, native, ordinary, and GC-stress
outputs. They are intentionally no longer corpus parity cases: Node and Bun agree, while both
frozen kiln CLIs disagree on the generator argc and mapped-arguments expectations. The CJS probe
also demonstrates the native gap directly without relying on VM behavior. Baseline and candidate
CLI outputs are identical for these unsupported cases, so this audit does not attribute the gaps
to the argc optimization.

The two native CLI binaries were:

- candidate, built from the fa45 arguments-length test source: SHA-256
  `00442167c4047b7ab3e78ad46a70849eb0d270b7788ea891275eb341b013babc`;
- pre-change baseline: SHA-256
  `af80860eda702b4d6d6742ca15e5b23013fe65649e759b0078aba1ffdef73d27`.

Both used frozen provider manifest digest
`4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`,
with `KILN_JOBS=1` and `RAYON_NUM_THREADS=1`. Candidate and baseline native runs exited 0 in
ordinary and `KILN_GC_STRESS=1` modes. Full output logs and checksums are under `raw/`.

The newly added CJS file is retained only as a standalone unsupported-behavior reproduction; it is
not in `conformance/native/`. The supported MJS corpus update has not yet been tested with a native
CLI after the added retained-object case; that is the next validation step on the active candidate
build.
