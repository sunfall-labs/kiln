# Diagnostic weak Array registry

This is private diagnostic evidence, not an upstream test or conformance verdict. It adds an
opt-in weak registry of `ArrayData` instances, with no collector-root or edge-accounting changes.
The scanner checks direct borrowed values in array items, collection-index keys, exotic elements,
and array properties. It upgrades one weak entry at a time, records the target's strong count
before and after traversal, and drops each temporary array owner immediately.

## Provenance

- Upstream pin: Effect `4.0.0`, `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
- Upstream `Pool.test.ts` SHA-256: `a219193dd30dc547f7364a723e2682d653ab824ccbda816b48b1a9d5bfb539f8`.
- Fixed private probe SHA-256: `dc0e5485b048b7f238d066a790a3681a804c5aa892ba10f5b11629a9f1af90b9`.
- Interpreter source base: `83fb70f86f2a103bf2fc7b3ab4f0be7292b1e583`.
- Private diagnostic diff SHA-256: `643250499fa2afa6fff02920b8ab4899f6cb2c69e5b9877905c380c415a88f4a`.
- CLI SHA-256: `85826c8f171270eb3be043039f0f72c220811e6b2a62bfa650aa22f9fe88ee40`.
- Probe output SHA-256: `283e6e60f75f74815c5464af1b59e97761a107e24471cb840831c0afee3df2d1`.
- Full raw output: `probe.log` in this directory.

The CLI was built with `MBX_GC_AUTO=false KILN_JOBS=1 RAYON_NUM_THREADS=1 mise exec -- mbx
build --release -p kiln-cli --jobs=1`. `ArrayData::new` is the sole constructor found by source
search; all current `ArrayData` allocations route through it.

## Observation and limits

At the repeated post-collection checkpoints, target Scope `0x7fd9c9640630` remains at
`strong=2,incoming=0`. A registered array at `0x7fd9c95ff6b0` directly holds it in `items[1]`;
that array has `strong=2`, is absent from the production collector graph, and has no production
incoming edge. The diagnostic's before/after target count is unchanged, so the weak-registry scan
itself did not add a persistent owner. A second non-node array similarly holds Scope
`0x7fd9c9617200` in `items[1]`. A control target is found in an array already present in the
production graph, showing the scanner also sees a normal accounted edge.

This proves a direct ArrayData-to-Scope strong edge that the current collector census misses. It
does **not** identify who owns the non-node array, prove that this is the only missing edge, or
show that the upstream Pool failures are fixed. The unchanged scoped probe still reports its two
resources alive. No production collector behavior was changed. The next diagnostic should inspect
actual direct parents of the non-node array through existing weak object, cell, closure, generator,
and array registries, one upgrade at a time, without cloning child values or feeding the collector.

## Ancestor trace

A follow-up one-worker build used the same private probe and added a cycle-safe upward trace over
arrays already in the weak registry. Source diff SHA-256:
`f7293ded58c7f2cedcee5d825a1953f0abbed4761efd37d8571472d3012fc985`; CLI SHA-256:
`5efe4b69760a185afaee4cb045e8391bf8f609e1a5a8b600ff81018ba466480d`; output SHA-256:
`a9767b26e3a41e8e88a0f871a46b668cd50a94896c77bf4b192fd4ee93a0765e`. The complete output is
`ancestor-probe.log`.

For one retained Scope, the observed chain was:

`Scope 0x7f5245640b40 <- Array 0x7f524560af90 (items[0]) <- ObjectData 0x7f526e1e5b30 (property[0].value)`

Both array and object are absent from the production graph and each reports `strong=2,
incoming=0`. The second retained Scope has the analogous chain:

`Scope 0x7f5245603eb0 <- Array 0x7f524563b530 (items[1]) <- Array 0x7f524563b330 (items[0]) <- ObjectData 0x7f52456235f0 (property[0].value)`

Again, the arrays and ObjectData are reported outside the production graph with no incoming edge.
The trace holds target counts unchanged across registry scans. It establishes an unaccounted
ObjectData-to-ArrayData-to-Scope path, but has not yet found who owns those ObjectData instances;
that owner chain is the next diagnostic target. The probe still reports the resources alive, so this
is not a Pool pass or a fix.
