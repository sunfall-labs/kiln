# 0010. Hash choice follows whatever compares the result

## Status

Accepted. Amended 2026-08-30 when protocol `Sync` and daemon socket discovery were found to use
FNV as their whole coherence or identity decision, while the implemented artifact cache had already
chosen SHA-256 rather than the proposed blake3 dependency. The rule survives unchanged; the
concrete strong digest is now the workspace's dependency-free, full-width SHA-256, shared by all
three consumers through `kiln-digest`.

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
thing compared, this needs to be a real 128-bit hash instead." The hashes that reach `Interface`
and `ModuleSummary` are carried *inside* values that early
cutoff compares with `PartialEq` (ADR 0003), so a collision costs a redundant recompute and can
never produce a stale result. FNV also has the one property that mattered for choosing it over
`DefaultHasher`: it is a fixed algorithm that means the same thing across Rust releases and
across the wire, and `matches_reference_fnv1a` pins it.

The original decision missed one wire use that does not have a value behind it. `Request::Sync`
lets a stateless client name every source while uploading only changed contents. The daemon has
only its held bytes and the client's digest; matching digests are the entire reason it retains the
held bytes. A chosen FNV collision therefore retained stale source. Protocol 13 replaces that
field with a full SHA-256 digest encoded as 64 lowercase hexadecimal characters. The readable FNV
module summaries remain unchanged.

Daemon discovery had the same defect. A canonical project root was reduced to 64-bit FNV and that
value alone selected the Unix socket. A chosen collision could therefore connect a client to another
project's live daemon; there is no full root behind the socket name for the client to compare. The
socket identity is now the full SHA-256 digest of the canonical root, encoded as 43 unpadded
Base64URL characters. The compact spelling and one-character socket filename keep the derived path
inside macOS's short `sockaddr_un.sun_path` limit without truncating the identity.

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
become visible if it were three times slower. The reasons for a strong digest are collision
resistance and a documented, versioned output format, not throughput.

The relevant thing ADR 0004 has to say has changed, which is why this ADR is possible at all.
Its zero-dependency rule for `kiln-db` rested on a false claim about incremental rebuild cost and
has been narrowed to what it was actually protecting: no procedural macros, nothing generic
enough to be re-instantiated at every use site. `rustc-hash` clears that easily — about 650 lines,
no build script, no non-optional dependencies, and `FxHashMap` is a type alias for std's
`HashMap` under a different `BuildHasher`. That is one more instantiation of map machinery the
crate already compiles, plus a hasher of a few multiplies — not a new generic layer. The eventual
strong digest takes the other permitted route: a small concrete dependency with no macros, generic
framework, external dependencies, or build script.

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

**Where a digest is the whole decision, use full-width SHA-256.** `Request::Sync` source coherence,
daemon socket discovery, and every content-addressed artifact key use the concrete
`kiln-digest::Digest`. They hash their inputs directly. They do not reuse FNV summaries, and no FNV
value is ever fed into a strong key; a strong hash over a weak summary would still have the weak
summary's collision boundary.

`kiln-digest` owns the pinned FIPS 180-4 implementation and reference vectors. It is deliberately
small and dependency-free. `kiln-proto` owns the source-specific newtype and its strict wire form;
`kiln-artifact-cache` re-exports the same digest type so its existing public API does not fork the
algorithm.

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

Three hash choices in one workspace is a cost we pay on purpose. Each has a one-line rule attached
to it — full-value-backed summaries, process-local tables, digest-only decisions — and they live in
separate places: `kiln-db::hash`, the Fx map type, and `kiln-digest`. Reaching for whichever hash is
nearby is how the unsafe version of this design returns.

Changing the strong algorithm later is a format change, not a refactor: artifact cache records pin
their format and `SourceDigest` pins the wire representation introduced by protocol 13. Likewise,
`ModuleSummary` still carries `u64` FNV values on the wire. Widening those summaries would need its
own protocol revision only if they ever stop being backed by whole-value equality.

The daemon socket spelling is likewise a discovery-format change. Kiln is still version `0.0.0`, so
this one pre-release upgrade requires stopping existing daemons before replacing the binary. The FNV
path is not a compatibility fallback: probing it would restore the collision boundary this amendment
removes and could connect to the wrong project. Once the strong path ships, it remains independent
of protocol version so ordinary client/server skew reaches `Ping` instead of starting another daemon.
