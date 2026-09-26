# Interned property keys and per-site inline caches in the C backend

Status: closed (landed on main: `5a072e20`, `0c7366dd`, `1c49e1f9`; ADR 0047 records the pre-interned static per prelude key as the next step)
Branch: `perf/interned-keys` (worktree `.claude/worktrees/agent-a762689d8490dad38`)
Owner: Claude (session 016duSRiZ6duBoS1bmWq7Yd1), 2026-09-02

## Problem

Every named property access compiled by `kiln-backend-c` reaches the prelude as a `(ptr, len)`
pair and is answered by a linear scan of the receiver's property list comparing bytes
(`kiln_c99_same_key`: pointer, then length, then first byte, then `memcmp`). The 2026-09-01/02
perf sessions attribute about 14% of a saturated request to the C object model, most of it in
that lookup. Two things make it slower than it needs to be:

1. **Key identity is not pointer identity.** A literal name is one static array *per translation
   unit*, so `req.url` in two modules is two pointers, and a computed key (`obj[name]`, a
   `JSON.parse` key, an `Object.keys` result) is a heap string that never matches any literal by
   pointer. The pointer fast path in `same_key` therefore hits only within one unit and only for
   keys stored by the same literal; everything else falls to `memcmp`, and every *mismatch* in
   a scan costs a length compare and often a byte compare too.
2. **Every access is a scan.** There is no per-site memory: `this.count` inside a loop scans from
   slot 0 each time, and a class method reached through the prototype scans the instance first
   and the prototype second on every call.

## Goal

Make key identity pointer identity, everywhere, so a lookup is a pointer scan; and give each
compiled named-access site a small cache so the common monomorphic case is one bounds check and
one pointer compare with no scan at all. Keep every observable semantic: property order,
`Object.freeze`/`writable`/`enumerable`/`configurable`, prototype walks, symbols (`@@`-keyed),
integer-indexed keys, and the collector's invariants (ADR 0041).

## Design (summary; the durable record is the ADR named below)

### Interning is total for stored keys

- The heap (`kiln_gc_heap`, one per realm, shared by every translation unit) owns an open-addressed
  **intern table** mapping bytes -> one canonical pointer.
- **Every** write of a `kiln_property.key` goes through the table. There are exactly two such
  sites in the prelude (`kiln_c99_own_slot` and the namespace wrapper) plus symbol creation, which
  registers its reserved key.
- `kiln_c99_intern(bytes, len)` inserts. If the bytes are a `KILN_GC_STRING` block of exactly that
  size it adopts the block as canonical; otherwise it copies into a fresh string block. Static
  bytes are registered with `kiln_c99_intern_static`, which uses the bytes themselves and **pins**
  the entry.
- `kiln_c99_interned(bytes, len)` looks up without inserting. Because storage is total, a miss
  means *no object anywhere has this key*, and the read paths use that to skip scans entirely.
- The memcmp fallback is **eliminated on the converted paths** (`get_own_kind`, `put_mode`,
  `own_slot`, `has_property`, `delete`, `own_private`, `write_would_accept`): they canonicalize the
  query once and then compare pointers. Unconverted cold paths keep `kiln_c99_same_key`, which is
  still correct against canonical storage (bytes-equal is still key-equal) and still has the
  pointer short-cut.
- `KILN_KEYS_CHECK=1` makes every converted scan re-run the byte comparison and abort with a
  message if the two disagree. It is the acceptance test for "interning is total", run under the
  native corpus alongside `KILN_GC_STRESS=1`.

### The collector

- The table is **weak** for unpinned entries: after marking, an entry whose canonical bytes are an
  unmarked block is dropped before the sweep frees the block. A live property keeps its key's
  block marked (the object tracer already marks `property->key`), so an entry is dropped only when
  no object holds the key.
- Pinned entries (static unit constants, prelude literals, symbol keys) are marked by the table
  during the mark phase when they are blocks (the heap-first-then-static case), and never dropped.
- `kiln_c99_gc_release` on a string block forgets its table entry, so explicit release cannot
  leave a dangling canonical pointer.
- The table is freed with the heap at teardown. The fallback (no-realm) heap never collects and
  its table only grows, as today's fallback heap already does.

### Compile-time interning

- The emitter collects every string constant a named-property instruction uses (`GetProperty`,
  `SetProperty`, `SetPropertyStrict`, `CallMethod`, `DeleteProperty(Strict)`,
  `LoadSuperProperty`) and emits `static const char *{symbol}_key_{n};` for each.
- `{symbol}_body` fills them once per realm with `kiln_c99_intern_static`; the unit's
  `KILN_GC_PHASE_RESET` (called at realm destroy for every registered unit) clears the ready flag
  so a new realm re-registers. A key that the runtime interned first (a `JSON.parse` key that
  matches a literal) is simply pinned; the canonical pointer stays whatever it was.
- Emitted sites pass `{symbol}_key_{n}` instead of `{symbol}_constant_{n}`.

### Inline caches

Objects have no shapes (the `kiln_object` comment records why: a shape identity has to be globally
stable and never recycled to be sound, and inventing one before there is something to guard is how
that goes wrong). The first step is therefore a **slot-guess cache validated by key identity**:

- Each `GetProperty`/`SetProperty`/`SetPropertyStrict`/`CallMethod` site gets
  `static kiln_c99_site {symbol}_site_{k}` holding `own` (a slot index on the receiver) and
  `proto` (a slot index on the receiver's immediate prototype).
- A read hits when `own < count && properties[own].key == key` and the slot is a data property.
  That check is exact without any object identity: keys are unique within an object, so the slot
  with this key *is* the property. Objects built by the same code path put keys at the same
  indices, so a monomorphic site hits across all instances of a "class" without a class existing.
- The prototype case requires proving the own miss (a pointer scan of the receiver, no memcmp),
  then the same slot check on the prototype. This is what a class method call costs.
- A write hits when the own slot matches, the property is a writable data property, and the
  object is neither frozen nor a module namespace.
- Sites never learn `@@` keys (machine-private, hidden-slot, symbol) or `__proto__`, so the slow
  path's hiding and relinking rules stay the only rules for those.
- **Next step** (not in this change): a shape/transition tree so the own miss for the prototype
  case is a shape compare rather than a scan, and so megamorphic sites can be detected and given a
  per-site hash instead of thrashing. The slot-guess cache is compatible with it: the shape adds a
  guard, the slot index is the same field.

## Out of scope

- Shapes / hidden classes (named above as the next step).
- Symbol keys as a value kind (they stay `@@symbol:<n>` strings; interning gives them pointer
  identity for free).
- The interpreter.
- Extracting the prelude into separate files (owned elsewhere; all edits stay inside the string).

## Verification

- `mbx test -p kiln-backend-c -p kiln-native -p kiln-cli --offline` (bare `cargo` here, see below)
- `kiln test conformance/guard`, `conformance/effect`, `conformance/regress` with the worktree's
  own binary and `--one-shot`
- `scripts/native.sh` with `KILN_BIN` pointing at that binary; then again with
  `KILN_GC_STRESS=1 KILN_KEYS_CHECK=1` on the native side (see issue 05 for how)
- `.scratch/perf/probe.mjs` interpreted vs native, diffed
- Perf: `.scratch/perf/app-props.mjs` compiled with `--profile native-release --node`, driven by
  `bench/server/drive-multi.sh 4 16 5000 18925 mixed`, three passes, same worktree, same session.
- `mbx clippy --workspace --offline`, `cargo fmt --check`

### Deviation from CLAUDE.md, recorded

`mbx` is not installed on this machine: it is not in `~/.config/mise/config.toml`, not in
`~/.cargo/bin`, and `find / -name mbx` finds nothing. Every `mbx` command in this PRD was run as
bare `cargo` under `nice -n 10` so it yields to concurrent builds. `cargo` is the pinned 1.97.1.

## Baseline (before any change)

Commit `8aec5b8ffdc947dfc2b791a2b6319d232f777a7b`, worktree binary
`target/debug/kiln` mtime 2026-09-02 14:34 (built by the test run below).

| Check | Result |
| --- | --- |
| `cargo test -p kiln-backend-c` | 59 passed, 1 ignored (unit); 25 passed (emitted_c) |
| `cargo test -p kiln-cli` | 123 + 1 + 5 passed |
| `cargo test -p kiln-native` | 86 passed, 2 failed, 2 ignored — `native_json_errors_are_catchable_and_uncaught_errors_are_reported` (Launch: "Text file busy", an environmental race) and `owned_result_scalar_api_copies_strings_and_is_idempotent` (symbol names carry an `@@KILN_1` suffix the assertion does not expect); both pre-existing on the untouched tree |
| `kiln test conformance/guard` | discovered 20, pass 20 |
| `kiln test conformance/effect` | 12 skipped: "generated bundle is missing, stale, or unverified; rebuild it with conformance/effect/build.sh" (bundles are gitignored and `node_modules` is absent) |
| `kiln test conformance/regress` | discovered 10, pass 9, fail 1 (`r08-node-async-context`: `require is not defined`) |
| `scripts/native.sh` | `native: 76 matched, 2 refused, 0 diverged` (refused: `generator-protocol` CallMethod(from), `shadow-realm` host realm facility) |
| perf `app-props.mjs` (binary mtime 14:36:36, `--profile native-release --node`, 4 procs × 16 conns × 5 s, load 1.04 at start) | pass 1 `rps 46235 p50 1.128 ms p99 3.88`; pass 2 `rps 46274 p50 1.122 p99 3.623`; pass 3 `rps 45096 p50 1.187 p99 3.998`; errors 0, wrong 0, rss_end 10400 KiB |

## After

Working tree at the final commit; binary `target/debug/kiln` rebuilt from it (mtime in
`final-meta.log` of the scratchpad, and stated in the final report).

| Check | Result |
| --- | --- |
| `cargo test -p kiln-backend-c` | 59 passed, 1 ignored (unit); 25 passed (emitted_c) — snapshots regenerated and reviewed at each step |
| `cargo test -p kiln-cli` | 123 + 1 + 5 passed (unchanged crate) |
| `cargo test -p kiln-native` | FINAL_KILN_NATIVE |
| `kiln test conformance/guard` | FINAL_GUARD |
| `kiln test conformance/effect` | FINAL_EFFECT |
| `kiln test conformance/regress` | FINAL_REGRESS |
| `scripts/native.sh` | FINAL_NATIVE |
| `KILN_GC_STRESS=1 KILN_KEYS_CHECK=1 scripts/native.sh` | FINAL_NATIVE_STRESS |
| `.scratch/perf/probe.mjs` interpreted vs native | FINAL_PROBE |
| `cargo clippy --workspace --offline` | **fails on main too**: `crates/native/kiln-native-provider-ffi/src/lib.rs:680` `Some(epoch + started.elapsed())` under `deny(clippy::arithmetic_side_effects)`; untouched by this branch |
| `cargo fmt --check` | clean |

### Perf: `.scratch/perf/app-props.mjs`, four client processes x sixteen connections x five seconds

First after-run (three passes, right after the corpora, load 1.21): `rps 42800 / 42243 / 43796`.
Interleaved base/after/base/after, two passes each, load 3.0-5.2 (other agents building):

| | pass 1 | pass 2 |
| --- | --- | --- |
| base round 1 | 46420 | 44272 |
| after round 1 | 41231 | 41532 |
| base round 2 | 43902 | 44053 |
| after round 2 | 42246 | 41977 |

**About 5% slower on this workload.** Per-operation microbenchmark
(`.scratch/interned-keys/bench-props.mjs`, native-release, ms, two rounds each, base -> after):
own read 32.5 -> 14.8; own write 114 -> 102; proto read 34 -> 21; proto method call 107 -> 75;
missing read 197-210 -> 162; computed read 85-91 -> 104-109; computed write 82-85 -> 105-113;
literal create (+ one computed read) 199-205 -> 236-247; json parse 182-196 -> 191-201; json
stringify 185-189 -> 191-200; keys 78-81 -> 77-81; `in` 48.6 -> 63.4; dict grow 324-339 -> 306-312.

Reading: the compiled-site paths (the point of the change) are 1.1-2.2x faster; every path that
arrives without a canonical pointer now hashes the key once, and for a small object that is more
than the byte scan it replaced. The server workload is dominated by the prelude building request,
response and URL objects with prelude literal keys -- none of which is a compiled site -- so it
lands on the losing side. Two mitigations are in: sites thread their canonical pointer through
`get`/`put_mode`/`own_slot` (`_ikey` forms) and hash nothing, and a new-key write interns once
instead of looking up and then interning. The remaining step -- a pre-interned static per
prelude literal key, validated against the realm -- is several hundred call sites and is recorded
in ADR 0047 as the next step rather than done here.

## Commits

- `942cb87` Intern property keys so a key comparison is a pointer comparison (issues 01-03)
- `c48fbe0` Give each named-access site a slot cache validated by the key (issue 04)
- (final) Thread a site's canonical key through get, put and own_slot; intern once on a new-key write
- (final) ADR 0047, NOTES entry, PRD and issues, microbenchmark
