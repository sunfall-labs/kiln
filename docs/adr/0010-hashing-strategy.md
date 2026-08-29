# 0010. Hash choice follows whatever compares the result

## Status

Accepted. Two of the three parts describe the tree as it should be now; the blake3 part is a
decision about *when*, and the when is §7's artifact cache, not today.

It is also where `kiln-db`'s first dependency is written down. Under ADR 0004 as corrected,
`rustc-hash` no longer needs an ADR to justify it; blake3, which carries a build script, will.

## Context

The project hashes things for two unrelated reasons, and currently serves both badly in
opposite directions.

`MemoEngine` interns paths in `by_path: HashMap<String, FileId>`, which is std's `HashMap` and
therefore randomly seeded SipHash-1-3. Both of the properties that buys are properties this
table does not want. DoS resistance protects a map whose keys an attacker chooses; these keys
are paths the compiler was pointed at by the developer running it, and an attacker who can
choose them can do considerably worse than degrade a hash table. Seed
randomisation exists so that no program's behaviour comes to depend on iteration order, which is
a property of std's contract rather than of this table, and one we could get more cheaply by
simply not iterating it. What is left is a keyed permutation run over every path lookup for no
benefit anyone here can name.

In the other direction, `kiln-db::hash` is 64-bit FNV-1a, written out by hand. Its module comment
argues the case correctly and then names its own expiry date: "if a hash ever becomes the only
thing compared, this needs to be a real 128-bit hash instead." Today nothing does. The hashes
that reach `Interface`, `ModuleSummary`, and the wire are carried *inside* values that early
cutoff compares with `PartialEq` (ADR 0003), so a collision costs a redundant recompute and can
never produce a stale result. FNV also has the one property that mattered for choosing it over
`DefaultHasher`: it is a fixed algorithm that means the same thing across Rust releases and
across the wire, and `matches_reference_fnv1a` pins it.

Design.md §7 is the trigger the comment describes. A content-addressed artifact cache reuses an
artifact "whenever its inputs have identical hashes" — key equality is the entire decision, and
there is no value behind the key to fall back on. Under §7 a collision is not a redundant
recompute; it is the wrong object linked into a binary, reproducibly, possibly on a machine that
never compiled the input. 64-bit FNV is unfit for that twice over. The birthday bound alone is
uncomfortable at cache scale — about one chance in forty million of an accidental collision at a
million entries, but one in four thousand at a hundred million, and §7's durable built-in and
dependency artifacts are exactly the entries that accumulate. More decisively, FNV is not merely
non-cryptographic but trivially invertible: a colliding suffix is a few seconds of arithmetic, so
any cache that ever spans more than one trust domain is poisonable by construction.

The argument this ADR is *not* making is a speed argument, and it is worth being explicit about
that because "swap the hash function" usually is one. Measured here on the `medium` reference
workload with an optimized build, the byte-at-a-time FNV loop runs at about 0.9 GB/s: 1.5 µs for
a single module of 1.2 KB, and 0.42 ms to hash all 320 modules and 373 KB of the project at
once. Against §13's budget of 200 ms for a warm small edit, hashing is not visible and would not
become visible if it were three times slower. blake3 is in fact the faster function in bulk, but
that is a bonus and not a reason. The reasons are collision resistance and a documented,
versioned output format.

The relevant thing ADR 0004 has to say has changed, which is why this ADR is possible at all.
Its zero-dependency rule for `kiln-db` rested on a false claim about incremental rebuild cost and
has been narrowed to what it was actually protecting: no procedural macros, nothing generic
enough to be re-instantiated at every use site. `rustc-hash` clears that easily — about 650 lines,
no build script, no non-optional dependencies, and `FxHashMap` is a type alias for std's
`HashMap` under a different `BuildHasher`. That is one more instantiation of map machinery the
crate already compiles, plus a hasher of a few multiplies — not a new generic layer. blake3 does
not clear the bar as easily, and the honest accounting is below.

## Decision

Hash choice follows what compares the result.

**Where a full value is compared behind the hash, FNV stays.** `content_hash`, `hash_names`,
`Interface::interface_hash`, and the hashes carried in `kiln-proto::ModuleSummary` keep 64-bit
FNV-1a exactly as written. They are summaries that make cutoff legible and tests readable; the
mechanism is `Interface: PartialEq` (ADR 0003), and a collision costs one wasted recompute. The
reference-vector test stays, because these values cross the wire and the algorithm is part of the
format.

**Where nothing leaves the process, use `FxHashMap`.** `MemoEngine::by_path` becomes
`rustc_hash::FxHashMap<String, FileId>`, and any later process-local intern or side table follows.
`rustc-hash` is already declared in `[workspace.dependencies]`; `kiln-db` opts in with
`{ workspace = true }` and stops being a zero-dependency crate, deliberately and for the first
time. The rule that comes with it is the one the workspace manifest already states: never for a
hash that is persisted or sent on the wire. Fx is fast because it is weak, it has no stability
guarantee across `rustc-hash` releases, and its per-byte behaviour is architecture-dependent.

**Where a hash is the whole decision, use blake3, and adopt it when §7 lands.** Every key of the
content-addressed artifact cache — the source→HIR, HIR→IR, IR→C and C→object edges of §7,
together with the namespace inputs that section lists — is a blake3 hash, truncated to 128 bits
for storage, which leaves a 2^64 birthday bound and is far more than a build cache needs. Cache
keys are computed by hashing the artifact's inputs directly. They do not reuse the FNV summaries,
and no FNV value is ever fed into a cache key; if that ever looks convenient, the FNV value is to
be replaced rather than wrapped, because a strong hash over a weak one is a weak hash.

We adopt blake3 at §7 and not before. Nothing in the tree today keys anything by hash alone, and
a dependency added early for a use that does not exist yet is one we cannot evaluate, since the
thing that would tell us whether it is configured right is the cache we have not built.

## Consequences

`kiln-db` has a dependency, and the comments that say it does not are now wrong. The crate
manifest and `MemoEngine`'s doc comment both advertise zero dependencies as a deliberate choice;
both should say what the choice actually is, which is no macros and no generic-heavy code on the
hot rebuild path. Leaving them as they are would teach the next reader the rule this ADR and
ADR 0004 just corrected.

Nothing observable changes when `by_path` switches hashers, and that is a claim worth keeping
true. `by_path` is only ever `get` and `insert`; `FileId`s are handed out in insertion order from
the `files` vector, so no output depends on hash order today. If something later iterates a map
whose order reaches a diagnostic, a generated file, or a cache key, that iteration gets sorted or
that map gets replaced with an ordered one. Fx's order is deterministic across runs, which makes
this failure mode easier to introduce and no less wrong.

Two hash functions in one workspace is a cost we pay on purpose, and three once blake3 lands.
The mitigation is that each has a one-line rule attached to it — persisted-and-compared,
process-local, key-only — and that they live in different places: `kiln-db::hash` for the format
hashes, a type alias for the maps, and the cache crate for blake3. A hash function reached for
because it was nearby is how the version of this decision that we are trying to avoid happens.

blake3 will need its own scrutiny at adoption time, and it is not free under ADR 0004's other
rules. blake3 1.8 ships a `build.rs` with a `cc` build-dependency to assemble its x86 SIMD
kernels, and ADR 0004 says a build script anywhere in this workspace needs an ADR first. This is
that ADR only in principle: the concrete decision — whether we take the assembly path, or the
`pure` feature that drops `cc` and the build script for portable Rust at some throughput cost, and
what that costs a clean build — is deferred to when we actually add it, with `--timings` before
and after. It belongs in the cache crate, which is a leaf, and not in `kiln-db`.

Moving the persisted hashes to blake3 later, if we ever want to, is a format change and not a
refactor. `ModuleSummary` carries `u64` hashes on the wire and `matches_reference_fnv1a` pins the
algorithm, so widening those fields is a protocol revision that needs its own decision. This ADR
deliberately does not make it: the summaries are compared against whole values, that is what makes
64 bits sufficient, and the day they stop being compared that way is the day the question is
worth reopening.
