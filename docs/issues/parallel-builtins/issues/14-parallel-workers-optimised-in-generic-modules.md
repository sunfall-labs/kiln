# 14. Parallel workers compiled optimised in generic-only modules

Status: resolved

Issue 13 made qualifying `parallelMap` sites lower in generic-only modules (large release modules
and `native-release-size`), but their worker code is compiled with the rest of the module at
`-O0`: measured 9,016 ms single-threaded against 3,307 ms for the same callback in a specialised
module (about 2.7x per element). Emit each qualifying callback's typed entry and chunk workers
into their own small compile unit built at the profile's optimising level, so a large module's
parallel sites run as fast as a small module's, without optimising the whole module.

## Acceptance

- The issue 13 probe's heavy callback in the large module within 10% of the small module's
  per-element time at 1 and 16 threads.
- Release build time of the large thirdparty bundles unchanged (they have no sites).

## Comments

2026-09-26, branch `work/parallel-workers-opt`.

**Where the level is chosen.** `kiln-native`'s `emit_graph_module_for_profile` sets
`EmissionPolicy::generic_only` for a release module past the specialization budget
(`release_module_uses_generic_bodies`), and the graph builders (`build_graph`,
`build_shared_graph`) expand each `CModule` into compile units -- the module source, or its
chunks when it has more than 500 functions -- with one `generic_only` flag per unit, which
`configure_release_module_compile` turns into `-O0 -fno-sanitize=undefined
-fno-function-sections` (generic) or `-O2` (specialized), and `release_module_compile_identity`
into the object key's options.

**What changed.** A new `EmissionPolicy::parallel_workers_unit`, set by `kiln-native` exactly
when a release module goes generic-only. With it (and a decoupled prelude), `render_module`
sends each admitted parallel clone (`_p`) and its eleven chunk workers to a separate buffer
instead of the module, and returns it as `CModule::parallel_workers`: `#include
"kiln_runtime_prelude.h"`, the clone (unit-private, the name it was emitted with) and the
workers, defined non-static under the module's link-visible symbol
(`kiln_module_path_<hash>_pmap*_<proto>`). Nothing else is needed: a clone is admitted only if
its text names no host, receiver, upvalue table, shape row or collector, so it and the workers
use only the prelude's inline helpers and libm (the heavy probe's unit links against
`fmod`/`floor`/`kiln_c99_typed_get`/`kiln_c99_typed_set` and nothing else). The module unit
declares the workers extern and, in a compact unit whose private prefix is `ku`, adds `#define
ku_pmap*_<proto> <module>_pmap*_<proto>` so the site's text is unchanged; a chunked module
already uses its real symbol, and its chunks' declarations are unchanged. The site -- guards,
pool call, and the generic call when the pool does not answer -- is untouched. `kiln-native`
pushes the worker unit after the module's own units as one more unit with `generic_only =
false`, so it gets `-O2` and the `o2` object identity, keyed by its own source digest. A module
with no qualifying site emits byte-identical C and no extra unit (asserted in the new test), so
every thirdparty bundle keeps its keys; `-p kiln-native` single-module `build` and
`build_shared_library` never set the flag. The generated-C cache payload gains the field
(`native-graph-emission-input-v4`). The workers' declared order and names come from one table,
`emit_graph::PARALLEL_CHUNK_KINDS`.

**Acceptance.** Probe: `conformance/native/parallel-large-module.js`'s bulk (lines 1-94) plus
one heavy site (`x => { let s = x; for (let k = 0; k < 2000; k++) s = s * 0.999 +
Math.sqrt(k + x); return s; }` over 200k elements) as `docs/issues/pworkers/large.js`, and the
same site alone as `small.js`; `--profile native-release`, `heavy-ms` over three runs, machine
load average 40-43 (other agents building):

| threads | large, base (e9ea3dba) | large, this branch | small, base | small, this branch |
| --- | --- | --- | --- | --- |
| 1 | 7233 / 7517 / 10613 | 1068 / 1142 / 1497 | 1069 / 1076 / 1083 | 1219 / 1142 / 1068 |
| 16 | 585 / 602 / 698 | 138 / 144 / 142 | 140 / 137 / 136 | 140 / 137 / 143 |

Best of three, large vs small: 1068 vs 1069 ms at 1 thread, 138 vs 136 ms at 16 -- within 10%
(the base large module was 6.8x / 4.3x slower). Output identical in every run. The small
module's build on this branch was a full object-cache hit on base's keys.

Thirdparty release build time (`--one-shot --node build --profile native-release`, a fresh
project directory -- so a fresh `.kiln/cache` -- per run, one warm-up then ABBA; release `kiln`
at e9ea3dba and at this branch). Generated C and app object are byte-for-byte the same size in
every run, and equal to issue 13's (lodash 9,860,903 / 16,831,448; babel 39,392,681 /
61,566,928; typescript 237,136,606 / 293,332,824): no bundle has a site, so no unit changes.
Timings (emit / compile, ms) are load, not the change -- load average 32-47 and memory
exhausted during the typescript runs:

| bundle | base 1 | new 1 | new 2 | base 2 |
| --- | --- | --- | --- | --- |
| lodash | 1093 / 38824 | 1064 / 43202 | 1074 / 27342 | 1158 / 27331 |
| babel | 10062 / 66347 | 10591 / 58135 | 9949 / 58379 | 11095 / 37710 |
| typescript | 59530 / 111136 | 124135 / 113514 | 57001 / 89667 | 62436 / 77513 |

**Verified.** `mbx test -p kiln-backend-c --offline`: 177 + 36 passed, 0 failed (new:
`a_generic_only_module_puts_its_parallel_workers_in_their_own_unit` -- the unit exists only
with the flag and a site, defines all eleven workers under the module symbol, compiles alone at
`-O2` against the prelude header, the module declares and maps them and no longer holds the
clone, site records are unchanged, and a module with no site emits the same bytes and symbol).
`mbx test -p kiln-native --offline`: 217 + 4 passed, 0 failed. `scripts/native.sh`: 238 matched,
1 refused (`shadow-realm`, pre-existing), 0 diverged -- including `parallel-large-module.js` at
parallelism 1, 2 and max through the worker unit. `mise run native-stress` not run (no
allocation, rooting or collector change).

**Risks.** A worker unit includes the whole prelude header (~124 KB of declarations and inline
helpers) for a few functions: about 0.4 s of `-O2` compile per module that has sites, paid once
per content key. The `#define` of `ku_pmap*` in a compact unit is textual; it is safe because
those names are generated and unique per proto, but a future emitter that spells a worker name
differently must go through `PARALLEL_CHUNK_KINDS`. `native-release-size` still emits
generic-only modules without the flag (workers stay in the module at its size level), by
choice: that profile optimises for size.
