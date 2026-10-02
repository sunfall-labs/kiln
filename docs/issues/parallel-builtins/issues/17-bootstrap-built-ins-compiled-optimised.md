# 17. Bootstrap built-ins compiled optimised

Status: resolved (specialized bootstrap on by default in release builds; `KILN_BOOTSTRAP_SPECIALIZED=0` opts out)

Found by issue 16: every built-in written in TypeScript (`kiln-builtins/src/*.ts` -- `%TypedArray%`,
`Date`, `structuredClone`, the error machinery, the parallel family's fallbacks, ...) is compiled
into every native binary as one bootstrap module, `@kiln/runtime-bootstrap.ts`, and that module is
emitted **generic-only** in every profile (`kiln-native`'s `native_builtin_bootstrap_module` forces
`EmissionPolicy::generic_only`) and, in `native-release`, compiled at **`-O0`** like a user module
past the specialization budget (`release_module_uses_generic_bodies`: more than 16 functions or 512
instructions; `configure_release_module_compile`). So a release binary runs its built-ins with no
typed or fast clones and no optimiser: a `%TypedArray%` callback method in a module too large for
the call-site loops of issue 16 still takes 80-150 ms per million elements, `indexOf`/`reverse`
65-80 ms, and `Date` formatting several microseconds per call.

The generic choice was made for size (a specialized bootstrap once put 0.9 MB on a hello world),
but at `-O0`; and the budget exists for build time of large *user* modules, which the bootstrap is
not: it is one module per toolchain and build configuration, cached by content, compiled once.

Make the bootstrap compile with specialized bodies at the release profile's optimising level in
release builds. Evaluate: exempting it from the budget; splitting it into units (by source file or
otherwise) so the compile is not one long unit; putting hot built-ins in an optimised unit (the
worker-unit pattern of issue 14). Decide what `native-release-size` and `native-dev` do.

## Acceptance

- In a generic-only (large) release module, `%TypedArray%` callback and element-loop methods on a
  1M `Float64Array` several times faster than before; TS built-ins such as `Date` formatting and
  `structuredClone` not slower.
- Hello-world startup time does not regress; binary size and the bootstrap's cold compile time are
  measured before and after and the cost is stated.
- `scripts/native.sh` 0 diverged; tests of the touched crates pass.

## Comments

2026-09-27, branch `work/bootstrap-optimised`.

**Options measured** (fresh cache, `native-release`, one 1M-`Float64Array` probe padded past the
budget so its own module is generic `-O0`; ms, best of 5 in-process; loaded machine, load 5-12):

| | base (generic, `-O0`) | generic, `-O2` | specialized, `-O2`, one unit | specialized, `-O2`, 1 MiB units |
| --- | --- | --- | --- | --- |
| bootstrap C / compile | 2.2 MB / 2.5 s | 2.2 MB / 26.4 s | 9.0 MB / 59.2 s | 12.2 MB / 10-11 units, longest 9.5 s (sum ~80 s CPU) |
| hello-world executable | 10,795,976 | 9,034,696 | 10,530,240 | 10,634,672 |
| `forEach` / `some` / `every` | 100 / 87 / 103 | 103 / 93 / 145 | 23 / 21 / 21 | 20 / 21 / 21 |
| `map` / `filter` / `reduce` | 127 / 154 / 85 | 113 / 110 / 70 | 36 / 27 / 20 | 31 / 23 / 18 |
| `reduceRight` / `find` | 82 / 85 | 72 / 78 | 22 / 23 | 22 / 20 |
| `indexOf` / `includes` / `reverse` | 70 / 68 / 84 | 62 / 60 / 71 | 6.1 / 6.5 / 9.6 | 5.9 / 6.5 / 8.8 |
| comparator `sort` 100k / `join` 100k | 77 / 24 | 76 / 21 | 29 / 14 | 29-34 / 15 |
| `Date#toISOString` / `toString` x100k | 791 / 662 | 533 / 524 | 493 / 478 | 543-619 / 574 |
| `structuredClone` of a ~0.9 MB graph | 7.0 | 9.4 | 7.1 | 6.5-7.2 |

Hello-world startup, interleaved 15x50 runs: base 2.61 min / 2.82 median ms; split specialized
2.54 / 2.68. No regression. The prelude (9-12 s cold) is still the critical path of a cold build.
The single unit has the budget's problem (a 59 s serial compile), and generic `-O2` costs 10x the
compile time for little gain. `Date` gains only ~1.3x: its cost is elsewhere (allocation, calls
through `rawGetTime`), so that is a separate issue. `structuredClone` does not move.

**Design chosen: specialized bootstrap, split into byte-balanced units.** The worker-unit pattern
(issue 14) would need a per-proto "specialize these" plan through `emit_graph`, which is being
rebased by the realms work, and a hot list to maintain. Splitting by source file is not possible:
the sources share one module scope. What exists is the chunked-module path (units of a
module past 500 functions). So:
- `EmissionPolicy::unit_chunk_bytes: Option<usize>` (backend): with `compact`, chunk the module
  whatever its function count, greedily by bytes of C in function order (chunk 0 counts the
  module's own text). It is mixed into the policy hash only when set.
- The chunked path had never run on specialized code. The receiver-shape caches (`_fn_N_recv_*`)
  were declared non-static by `lib.rs` but defined `static` by `body.rs`. Now `lib.rs` declares
  them `extern` and the owning boxed clone defines them once.
- `kiln-native`: `native_builtin_bootstrap_module(.., specialized_unit_bytes)`. A release build
  passes `RELEASE_BOOTSTRAP_UNIT_BYTES` (1 MiB), and each bootstrap chunk is compiled as its own
  unit (`-O2`, `o2` identity, keyed by its own digest). `native-dev`/`check` stay generic. So do
  `native-release-size` (it optimises for size; the specialized C is 4-5x larger) and
  `native-profile` (not compact, so it cannot chunk: one 60 s unit).

**Blocked: a pre-existing specialized-emitter miscompile, so the default is off.** Before the
coordinator stopped it, a one-off run of `conformance/native` with every probe at `native-release`
hung on three probes for over two hours at 100% CPU: `web-headers-list.mjs`,
`app-serve-header-list.mjs` and `node-web-standards.cjs`. The cause is `web.ts`'s
`headerListRemove` (`Headers.set`/`delete`). The same function in a *small user module* (so it is
specialized) miscompiles on base `89852412` too, and a harness guard shows `from` stays `0`:

```js
function headerListRemove(list, needle) {
  let at = list.indexOf(needle);
  if (at < 0) return list;
  let kept = "";
  let from = 0;
  while (at >= 0) {
    kept += list.slice(from, at);      // without this line it is correct
    const end = list.indexOf("\n", at + needle.length);
    if (end < 0) return kept;
    from = end;                         // lost: the next indexOf searches from 0 again
    at = list.indexOf(needle, from);
  }
  return kept + list.slice(from);
}
headerListRemove("\nx-dup:1\ncontent-type:text/html\nx-dup:2\nz:1", "\nx-dup:"); // never returns
```

Removing the early `if (at < 0)` or changing the returns does not fix it. Only the
`kept += list.slice(from, at)` line matters. The generic-only bootstrap was hiding this; any
release user module with the shape is wrong today. It is in `emit_graph`, so it is left to that
owner. The mechanism is committed but gated: a release build specializes the bootstrap only under
`KILN_BOOTSTRAP_SPECIALIZED=1` (`release_bootstrap_specialized`). Flip the default once the
miscompile is fixed, and after a clean `native-release` pass over `conformance/native`. That pass
was not completed here: it stalled on the three probes above, so the rest is not known.

**Verified** (rebased on `4e7d27c7`): `mbx test -p kiln-backend-c --offline` 190 + 36 passed;
`mbx test -p kiln-native --offline` 229 + 4 passed. New tests:
`a_specialized_unit_split_by_bytes_compiles_and_links_as_units` (one function per unit, each
compiled with `-fno-common` and relocatably linked; it fails without the `body.rs` linkage fix)
and the specialized entry in `bootstrap_emission_cache_reopens_and_separates_surface_and_policy`.
`scripts/native.sh`: 269 matched, 1 refused (`shadow-realm`, pre-existing), 0 diverged (default
off, so release behaviour is unchanged). With the opt-in, `web-headers-list.mjs` still hangs
(timeout), as expected.

**Risks when enabled.** Cold cost is about 80 s of CPU (10-11 `-O2` units) per bootstrap
variant. The emission policy changes the bootstrap's C (`node`/URL surface, host clock, entropy,
timezone, locale, `--no-kiln-extensions`), so each configuration a machine builds pays once. On
a 4-core machine the bootstrap, not the prelude, would be the cold critical path, at about 20 s.
Cross-unit calls are not inlined. The measured cost of that is small (`reduce` 18-34 vs 20 ms).

2026-09-27, branch `work/release-miscompile` (issue 18).

**Unblocked, and on by default (`793ae8f3`).** The hang was not a lost store: a versioned body's
generic clone read the unboxed twin of a *speculated* value it had computed boxed, so after the
fast clone exited at `kept += list.slice(...)` the `indexOf` argument was a stale `0.0`
(`529c7c86`; details on issue 18). The `native-release` pass over every `conformance/native` probe
that this issue asked for then found three more faults the specialized bootstrap exposed, now
fixed: a `Date` receiver matched an empty receiver-shape row and crashed every `Date` comparison
and string conversion (`9072a86c`), typed-array stores of BigInts and objects (`d10e7996`), and
the `clz32`/`imul` intrinsics (`644226e8`, `539bef64`); and one backend fault in every profile
that the release source DCE exposed (`yield yield* x`, `c1d557ee`).

The pass now answers identically with the bootstrap specialized, generic, and specialized with
every speculation forced to exit (`KILN_SPECULATE=exit`): 265 of 273 match; the rest are
pre-existing release-only faults of user modules, the same with the compiler before issue 18, and
filed as issues 19 (the release source DCE drops coercions: 4 probes), 20 (integer remainder loses
`-0`: 1 probe) and 21 (a typed method call refuses a receiver its typed clone cannot take: 3
refusals), plus `shadow-realm`, refused everywhere. None of them involves the bootstrap, so the
default flips; `KILN_BOOTSTRAP_SPECIALIZED=0` restores the generic `-O0` bootstrap for bisecting.

The costs are the ones measured above (the specialized bootstrap's first compile is ~80 s of CPU
in 10-11 `-O2` units per configuration, then cached; hello-world startup unchanged); these fixes do
not change them.

**Verified** (branch head `793ae8f3`): `mbx test -p kiln-backend-c --offline` 195 + 36
passed, 1 ignored (the ADR 0036 migration gate, as on main); `mbx test -p kiln-native --offline`
229 + 4 passed, 2 ignored (the Fastify probes, as on main); `scripts/native.sh` 276 matched,
1 refused (`shadow-realm`), 0 diverged; with the release CLI and the new default,
`kiln test conformance/regress --node` 38/38 and `kiln test conformance/server --node` 6 passed,
0 failed, 2 skipped (fastify/express: packages not installed in this worktree).
