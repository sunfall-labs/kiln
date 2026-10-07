# 10. Ownership transfer

Status: resolved
Blocked by: 05; 08 for the read path it shares; 12 for the object-graph API

ADR 0070's primitive C: move a group of blocks from one realm's heap to another's by relinking them,
without copying.

1. **Transfer lists first**: `postMessage(buffer, [buffer])` moves an `ArrayBuffer`'s backing store
   and detaches it in the sender, exactly as Node does. This makes 06's transfer lists zero-copy.
2. **ADR 0069's worker arenas**, which join the realm's heap at the end of a parallel call
   (`docs/issues/parallel-builtins/` issue 08 builds on this).
3. **Object graphs** between realms, once a proof exists that the sender keeps no reference into
   the graph. The unsound name-matched `isolate` escape check in `kiln-hir` was removed; the
   runtime proof controls native handoff.

## Acceptance

- Transferring a 100 MB `ArrayBuffer` between realms costs the same as transferring 1 KB.
- `mise run native-stress` clean across transfer probes.

## Comments

### 2026-09-29 -- candidate-scaled transfer shipping acceptance

Production admission uses candidate-owner witnesses and independent context/root authorities;
canonical keys and native shapes cannot borrow candidate storage. Strict full-scan audit agrees
on the public transfer subset and fixed-candidate scaling cases. The 100 MB versus 1 KB buffer
handoff acceptance was recorded earlier; this batch completes the incoming graph proof.

Frozen source `6e9cc7a8` passed 2,323 workspace tests in 93 suites, with no failures and nine
ignored tests; native parity matched 362 probes with one existing ShadowRealm refusal and no
divergences. Routine Test262 passed 97,118 of 102,522 selected variants with no regression
across 122 chunks; 116 extended variants remain deferred. Node passed 39/39, server six with
two skips, and regress 49/49. The gate's only failure was a dayjs timing measurement; the isolated
performance lane passed all 12 present bundles on identical source and CLI with the unchanged
baseline. Both results are retained, rather than describing the original aggregate as green.

Full collector stress passed: 318 matches, one existing refusal, zero divergences and 44 explicit
skips. Strict-oracle native parity matched eight affected probes. Those probes matched twice at
1, 2, 16 and 64 realms; a three-probe subset also matched twice at 1, 2 and 16 realms under
collector stress. Source, index and CLI were verified unchanged throughout. Durable records:
`../performance-evidence/graph-ledger-shipping-verification-2026-09-29.json`.

The package test now checks successful closed-graph transfer and rejects reuse of a consumed
ticket, with actor cleanup in `finally`. Rejected entry modules report their original error even
with referenced actors or timers; two CLI regression tests pass, and the original hang reproducer
exits with its assertion error in 0.23 seconds.

Issue 08's broad bootstrap sharing stays deferred. Additional candidate-size and outside-owner-size
curves are performance characterization, not a claim of constant-time proof for arbitrary graphs.

### 2026-09-29 -- property-store follow-up passed integration and collector stress

The frozen batch at `99dc0f36` passed the routine gate: 90 crate suites, 2,317 passed,
zero failed and nine ignored; 361 native matches, one existing ShadowRealm refusal and
zero divergences; 17 thirdparty matches, zero divergences and five unavailable bundles.
Routine Test262 verified 102,522 selected variants, 116 deferred and 122 chunk baselines,
with 97,116 passing and no regression. Node passed 39/39, server 6/8 with two declared
skips, regress 48/48, and all 12 present performance bundles stayed within their baselines.
This is routine coverage, not extended Test262.

Full collector stress on the same source and CLI reported 317 matches, one existing refusal,
zero divergences and 44 explicit skips. The source, index and binary hashes were verified
unchanged through both runs. Durable aggregate and hash evidence is in
`../performance-evidence/graph-ledger-property-stores-gate-2026-09-29.json`.
These checks cover CommonJS root authority, one generated module-root inventory, generated
property-cache witnesses, and template/namespace value publication. Candidate-scaled production
admission and the remaining metadata authority work are still open.

### 2026-09-29 -- generated templates and namespace snapshots record value stores

Tagged-template raw/cooked arrays, their properties holder, and the namespace snapshot's tagged
value fields now use the same owner-carrying store helpers. The namespace fixture constructs a
snapshot retaining a sealed candidate export, requires an outside-owner witness and a matching
full-scan alias, then releases the fixture edge and requires clear agreement. Generated template
and namespace source guards cover direct array fields and descriptor aliases. Canonical key and
other pointer metadata coverage remains a separate obligation.

Two namespace fixtures passed, the generated-store guard passed, all 37 emitted-C checks passed
before the final cooked-array properties helper change, and the final-source native subset matched
four probes with zero divergences. Collector stress matched three eligible probes with one explicit
skip. The namespace snapshot was re-recorded automatically. Detailed hashes and check scope are
in `../performance-evidence/graph-ledger-generated-construction-verification-2026-09-29.json`.
The final integration gate covers the complete frozen batch. Production admission remains the
full scan while indirect outputs and metadata coverage are audited.

### 2026-09-29 -- generated property-cache writes retain ownership witnesses

The emitted compact monomorphic and polymorphic cache-hit paths bypassed the store authority,
even though the runtime property path used it. Both now supply their object owner to
`heap_store_val`. The public probe warms a two-shape write site, stores a candidate through each
cache, releases the original escape, requires refusal, then clears the holder and retries.

The previous CLI reproduced two strict-oracle disagreements (`ledger clear, heap alias`) and one
native parity divergence. The fixed CLI reports three matches, zero divergences and no audit
diagnostics. The actor probe matched at eight and sixteen realms, twice per count. An emitter
invariant check exercises compact decoupled emission and guards both owner-carrying paths. Source,
binary and log hashes are recorded in
`../performance-evidence/graph-ledger-generated-cache-verification-2026-09-29.json`.
Production admission remains the full scan; other generated construction and metadata stores are
still being audited before candidate-scaled admission.

### 2026-09-29 -- CommonJS context authority and one module-root inventory

Require constructors now carry the generated launcher's registered visitor and stable anchor.
The active provider validates the exact live registration without invoking a callback; unregistering
revokes it. Independent root inspection visits module cells once. Null default contexts are clear;
opaque legacy contexts without authority remain unknown. Ordinary and embedded graph launchers
register their anchors before constructing require objects. Both GC marking and transfer inspection
now consume one generated root visitor, eliminating duplicated root field lists.

The public native probe moves an unaliased graph after `createRequire`, refuses a retained module
export, and succeeds on retry after clearing the export. Strict audit reported three native matches
and zero divergences. Two probes matched at eight and sixteen realms, twice per count. Collector
stress matched two eligible probes with one explicit direct-factory provenance skip. The touched
crate suites passed 639 tests including 36 emitted-C checks, with three ignored; the subsequent
marking adapter passed the executable ordinary/shared launcher test. Detailed scope and source
hashes are in `../performance-evidence/graph-ledger-commonjs-verification-2026-09-29.json`.
This is focused evidence, not a new full gate. Sender-wide production admission remains in place.

### 2026-09-29 -- an opaque CommonJS context is a remaining incoming authority

The compiler-derived inventory found the require object outside the incoming kind inventory.
`../probes/actors-direct-commonjs-context.mjs` moves an unaliased plain graph after
`createRequire` in the interpreter. Native strict audit refuses with “ledger clear, heap
unproven”. The full scan remains safely conservative; this is an unresolved supported-context
limit. `../commonjs-transfer-context-design.md` proposes forwarding the generated launcher's
existing typed root visitor and its live anchor, rather than declaring an opaque context safe
or duplicating the module-root field list. Ordinary wrappers, `createRequire`, shared wrappers
and teardown must be audited together. Raw reproduction hashes are retained in
`../performance-evidence/graph-ledger-commonjs-gap-2026-09-29.json`.

### 2026-09-29 -- callable storage aliases use the same reference authority

The ordinary `function.prototype = value` path obtained a pointer to the
closure's prototype slot and wrote through it directly. It now supplies the
closure owner to `heap_store_value`; shared region closures continue to use
the separately enumerated realm root. Assigning `undefined` retains the
existing hole sentinel and observable prototype semantics. Complete property
descriptor commits also use `heap_store_property`, preserving the canonical
key through a local descriptor value.

A source guard checks immediate aliases returned by the callable prototype,
callable store and exotic-store accessors within each function. It distinguishes
pointer declarations and reads from writes. This extends the field-name guards;
it does not claim coverage of copied aliases or arbitrary output parameters.
An executable fixture writes a candidate through the actual callable property
path, requires the ledger and full scan to detect it, clears the slot and
requires both to become clear. The public actor probe retains a post-factory
alias through a callable prototype, requires refusal, clears it and retries.

Verification: 282 backend unit tests passed, one ignored; all 36 emitted-C tests
passed; the ten source guards passed. Three selected native probes matched;
two eligible stress probes matched with one explicit direct-factory provenance
skip. The public actor probe had no strict-audit disagreement diagnostic and
matched at eight and sixteen simultaneous realms, two runs per count. Evidence
is in `../performance-evidence/graph-ledger-callable-slot-verification-2026-09-29.json`.
These are focused follow-up checks, not a new full program gate. Production
admission remains the full outside-heap scan.

### 2026-09-29 -- shared store and incoming-owner foundation passed the integration gate

The frozen foundation at `7830873d` passed `mise run gate` in 612 seconds:
90 workspace suites, 2,310 passed, zero failed and nine ignored; 360 native
matches, one existing ShadowRealm refusal and zero divergences; 17 thirdparty
matches, zero divergences and five unavailable bundles. Routine Test262 verified
102,522 selected variants, 116 deferred variants and all 122 chunk baselines,
with 97,116 passing and no regressions. The Node corpus passed 39/39, server
6/8 with two declared skips, and regress 48/48. This is routine coverage, not
an extended Test262 run.

Full native collector stress on the same source and CLI reported 317 matches,
one existing refusal, zero divergences and 43 explicit stress skips. The source
snapshot and executable hashes were checked unchanged before recording evidence
in `../performance-evidence/graph-ledger-foundation-gate-2026-09-29.json`.

This lands the shared store authorities, publication inventories, weak owner
lifetime handling and strict full-scan/ledger oracle. Production admission still
uses the full outside-heap scan. Completing reference and metadata coverage and
replacing sender-wide certification with the candidate-scaled proof remain open.

### 2026-09-29 -- filled nursery membership and post-factory aliases

Filled nursery chunks now have explicit membership and predecessor links. Direct capsule
detach and general heap moves unlink each selected chunk without searching unrelated chunks;
adoption and collection preserve the same list invariant. A fixture moves two nonadjacent
chunks around unrelated neighbors and checks membership, links, payloads and heap accounting.
Prepared clone arenas clear their temporary indexing heap's membership before publication.

The public direct-graph probe now moves a factory-exposed alias into a new outside holder after
the factory returns. Transfer refuses while that holder retains it, leaves the holder usable,
and accepts the same ticket after it is cleared. Both engines pass. This requires any future
escape ledger to track stores throughout the candidate lifetime, not only during its factory.

Verification: 258 backend unit tests passed (one ignored), all 36 emitted-C tests passed, and
native collector stress reported 317 matches, one existing refusal, zero divergences and
42 explicit stress skips. Direct-graph, cancellation and concurrent-stop probes each matched
at 16 and 64 simultaneous realms, three runs per count. Logs remain in this worktree's
`target/graph-full-list-*.log`.
Candidate discovery and incoming proof still scan the sender heap; standalone large blocks
and candidate-scaled certification remain open.

### 2026-09-29 -- prepublication cancellation observes both owner callbacks

The provider's coordinator fixture now cancels with zero, one and both callbacks
entered, before `start` authorizes any C mutation. It waits on the actual readiness
condition rather than a sleep, then requires every ready or late callback to return
unavailable and the C control sequence to remain empty. The successful fixture still
requires the complete stage/reserve/detach/adopt/consume sequence. Both tests use real
owner-thread realm entry and share the registered control callback safely.

The shared host reservation test also closes each end of a claimed call's reply
channel before obtaining a publication lease. Both cases revoke the grant and
prevent publication. The complete touched crates passed: 58 provider tests and
213 host-runtime tests. This covers coordinator readiness and shared preflight;
the public abort/closure/publication cases remain in the ordinary corpus. The
new test changes passed the fixed-snapshot program gate on `a2444203` (2,288
workspace tests, 359 native matches, no regressions). Candidate-scaled proof and the
remaining direct storage classes remain separate implementation requirements.

### 2026-09-29 -- full gate and native collector stress passed

The socket-capable gate and native collector stress passed on `e4bda4df`.
Direct graph, cancellation and concurrent actor-stop probes matched at 1, 2
and 16 simultaneous realms, three runs each. Exact counts and coverage are in
`../program-verification.md`. Coordinator prepublication cancellation coverage
and the remaining graph proof/performance acceptance are still open.

### 2026-09-29 -- a stopped actor leaves an unpublished graph ticket usable

The public cancellation probe now creates a third ticket before stopping its actor, calls that
stopped reference and requires `ActorClosedError`, then transfers the same ticket successfully
to a new actor and checks the value. Both engines pass. Together with the existing snapshot
abort, publication abort and retained-alias retry cases, this establishes ticket reuse before
posting or after actor closure and consumption after irrevocable publication. Coordinator
prepublication cancellation races, the final socket-capable gate and full native stress are
still separate requirements.

### 2026-09-29 -- literal ownership has no material cost in the measured workloads

Two native-release variants from `e96cd536` used the same runtime and differed only in string
constant emission: the production candidate-literal helper versus the earlier static pointer
load. The final emitter was restored and the CLI rebuilt after constructing both binaries.
Three alternating rounds of the existing warm `bench/non-io/string-ops.mjs` gave **13.5 ms**
median for either variant, with checksum 300,000. Three alternating five-sample rounds of the
public ~1 MB actor graph benchmark gave production/static direct construction **5.55/5.61 ms**,
handoff and reply **14.29/14.10 ms**, and construction through reply **19.85/19.72 ms**.
Every graph path returned checksum 21,202,207. These rows do not establish a useful advantage
from complicating literal storage. The process load was 1.8--1.9 for the string workload and
5.5 for the graph comparison on 16 hardware threads; this is a scoped comparison, not the
quiet-machine audit of all earlier runtime changes. Raw data stays in
`target/literal-load-paired.json` and `target/literal-actor-paired.json`.

### 2026-09-29 -- cancellation probes distinguish posting from publication

A snapshot getter can synchronously abort an actor call. The actor module now checks settlement
after snapshot preparation and returns before posting the invocation; the unsuspended ticket
remains usable. The public cancellation probe retries that ticket successfully. Its publication
case now invokes a method with an unresolved result and forces actor shutdown at the end, so a
successful reply cannot race ahead of the injected abort. Previously that legitimate first reply
occasionally won under corpus load, making the probe report that its abort was not observed.

The revised probe passes in both engines, in 16 interpreter runs with eight concurrent processes,
and at 1, 2 and 8 simultaneous native realms. The builtins crate passed all 107 tests. The full
native parity run on the preceding literal change reached the known sandbox-blocked
`app-bun-serve-global` probe (unsettled top-level await); it is not a complete corpus verdict.
The final socket-capable gate and full native stress remain required.

### 2026-09-29 -- direct factories own generated string literals

Generated string loads now copy their static module string into candidate nursery storage only
while an `isolate(factory)` allocation scope is active. Ordinary loads keep their static pointer.
The candidate proof still rejects all unproven pointers outside the graph. A boundary conversion
had also replaced candidate-owned empty strings with the runtime's static empty string; it now
preserves any non-null string header, including zero-length ones. The public direct-graph probe
passes with Latin-1, UTF-16, empty strings and string arrays in both engines. Backend tests passed
257 unit cases and 36 emitted-C snapshots. The native stress rollback probe passed, and the
direct public probe matched with 1, 2 and 8 concurrent realms. The full native corpus and final
gates remain due.

### 2026-09-29 -- direct numeric object elements carry candidate provenance

A shaped object's numeric keys live in `kiln_object.elements`, a malloc side buffer. Growth in
the ordinary write, computed-definition and property paths now allocates an owner, capacity,
candidate-ID and generation header when that object was created in an actor candidate. The
outgoing certificate checks the header, its complete allocation span and every non-hole value;
the incoming proof sees the same span, and dictionary conversion and GC finalization free the
header rather than the interior elements pointer. Ordinary objects retain their plain malloc
layout. `conformance/native/actors-direct-graph.mjs` now transfers numeric keys 0 and 127,
checks key order after an actor allocation burst, and matches the interpreter. The focused
`../probes/actors-direct-numeric-keys.mjs` succeeds natively too. Backend tests passed 257 unit
cases and 36 emitted-C snapshots; the native-only direct rollback probe passed with
`KILN_GC_STRESS=1`. The public direct probe matched its lone output at 1, 2 and 8 concurrent
native realms.

The first reproduction also stored string literals as values and still refused after the
numeric side buffer was certified. Those literals point outside the candidate GC graph; the
strict walker has no process-owned immutable-string certificate for them. A direct factory
must either build those strings in candidate storage or gain a validated shared-string
authority. `prepareTransfer` remains the supported path for such values.

### 2026-09-29 -- terminal publication and cancellation use one ticket rule

`conformance/native/actors-direct-graph-cancel.mjs` aborts while the private host operation
returns terminal graph status 4, before the actor module consumes the ticket. Both engines
reject the call as aborted and refuse a second use of that ticket. The actor module defers
reentrant abort and reply-close completion during claim and status operations, then observes
the terminal ownership state before settling. A pending cancellation gets one scheduled
recheck and otherwise waits for the host's terminal wake.

The direct parity probe also covers sparse arrays and retries a ticket after a retained root
alias is released. Both engines pass. The numeric-key direct-object refusal found next is
addressed in the comment above.

### 2026-09-29 -- one graph transaction for both engines

The interpreter now participates in the host actor reservation used by the native graph
coordinator. Both engines use the same grant, placeholder, sender claim, publication lease,
receiver authentication, release, and terminal ticket status. The interpreter proves its
`Rc` graph exclusive and stores a plain token wire in the reservation; native continues to
relink certified collector chunks. The receiver decodes the interpreter wire only after the
grant is published. The host retains it until receiver release so assignment can retry.

`conformance/native/actors-direct-graph.mjs` covers successful direct graphs, cycles and
repeated references, ticket reuse, retained root and child aliases, and weak aliases in both
engines. The native-only collector-stress rollback probe remains separate. Broader layout
coverage, explicit cancellation races, large graph performance, and the final gates remain due.

### 2026-09-29 -- interpreter direct-transfer ownership boundary

`../interpreter-graph-transfer-design.md` specifies the remaining interpreter semantic-parity
path. Its `Rc` graph cannot cross worker threads directly. A strict strong/weak ownership
certificate over the existing plain encoder can reject sender aliases; accepted graphs can use
the thread-safe message tokens with ticket rollback until posting succeeds. The interpreter
would copy for semantic parity while native keeps its zero-copy chunk relink. This design is
not an implementation or a completion claim; the certificate, delivery integration and probes
below remain due.


### 2026-09-29 -- use validated candidate chunk bitmaps for membership

The outgoing certificate previously copied every candidate block address into a private
pointer hash set, even though `chunk_sealed_for_move` had already validated each candidate
chunk's complete contiguous bump layout and bitmap. The walk now checks a GC-validated block
against the exact claimed chunk list and its block-start bitmap. Separately allocated large
inline strings must still appear in the factory's validated large-block list. The root walk,
all-block edge validation, reached set, side-buffer certificate and incoming-alias scan remain
unchanged. No GC mark is written by this proof.

Seven alternating native-release runs of the same public ~1 MB actor graph and ten paired calls
per run gave matching checksums. Direct handoff and reply changed from **16.17 to 14.68 ms**
median; prepared handoff stayed at 5.13/5.18 ms. Direct construction through reply was
22.42/20.09 ms, though its construction phase also varied in these runs. The raw rows are in
`target/actor-graph-members-paired.jsonl`. The backend suite passed 257 unit and 36 emitted-C
cases. The realistic, multichunk and direct-transfer probes matched at 1, 2 and 8 realms;
the direct-transfer rollback probe matched at those counts under every-allocation collector
stress. Interpreter direct-factory parity and the final program gates remain open.

### 2026-09-29 -- reserve validated direct-proof tables before walking the graph

A temporary phase timer on the public ~1 MB `actor-graph` workload measured 15 successful
direct certificates: outgoing candidate validation took a 4.20 ms median, the outside-heap
incoming scan 1.44 ms, and C/provider root checks under 0.05 ms together. An exact chunk-range
prefilter before incoming pointer lookup was tried and reverted: ten paired runs measured
17.26 ms direct handoff before and 17.30 ms after, so it did not improve the public path.

The outgoing walk builds two private pointer sets. It now reserves `members` from each fully
validated chunk's allocation count before inserting that chunk, and reserves `reached` from the
validated total before following edges. The same exact outgoing and incoming certificate checks
remain. Eight paired runs with temporary phase timing lowered the outgoing median from 4.27
to 3.43 ms and direct handoff from 17.47 to 16.13 ms. After removing the timer, eight paired
native-release runs on the exact final source measured direct handoff **17.55 to 16.37 ms**
with matching checksums; prepared handoff remained about 5.2 ms. Raw rows are in
`target/actor-graph-proof-{reserve,final}-paired.json`; the temporary timing code was removed.

Backend C passed 257 unit cases and 36 emitted-C cases. The realistic graph and the direct
success/alias-refusal probes matched at 1, 2 and 8 realms. The direct rollback probe also
matched at those realm counts under every-allocation collector stress. Direct proof remains
more expensive than the prepared path; interpreter direct-factory parity and the full program
gates remain open.

### 2026-09-29 -- prepared ordinary objects retain numeric own keys

`prepareTransfer` now copies an ordinary object's numeric properties and named properties into
one arena-owned in-block dictionary. Numeric keys are generated inside the private arena, in
ascending index order; named keys follow their source order. The values use the existing clone
identity table, so nested objects, aliases and cycles keep their snapshot semantics. This adds
no candidate-only side-buffer header or sender-heap scan. The interpreter's plain-graph inspector
accepts the same objects and orders index keys before named keys; its private snapshot still
uses structured clone. The normal native message codec keeps its guarded fallback for numeric
objects.

The public actor probe builds indices 127 then 0 around a named field, mutates the source after
preparation, and has the receiving actor append index 255. Interpreter/native parity matched,
including the expected key order and snapshot values. Backend C tests passed 257 cases with one
ignored and 36 emitted-C snapshots; interpreter tests passed 203 with one ignored. The focused
probe matched under every-allocation GC stress and at 1, 2 and 8 simultaneous realms both
normally and under stress. The full native-stress corpus and program gate remain due.

### 2026-09-29 -- direct candidate shaped slots move with their object

A shaped object that grows beyond its inline or in-block value slots during `isolate(factory)`
now keeps candidate-only owner, capacity and generation metadata on the separate slot run. All
five normal resize call sites share this allocation path. Conversion to a dictionary and the
object finalizer free its allocation header; ordinary objects retain their existing slot layout.
The outgoing certificate validates the candidate's slot owner and shared shape, and its incoming
proof covers raw pointers into the entire slot allocation.

A native-only public actor probe moved a 12-property shaped object made by successive writes.
The receiver read its values, appended five more fields across another slot resize and returned
the checked 17-property result. A focused C fixture checked allocation, resize, side-span
classification, an outside owner alias and a forged candidate ID. Backend C unit tests passed
257 cases with one ignored. The prelude grew to 65,075 lines for this audited storage path, so
the size ceiling moved to 65,500 after inspecting the emitted surface; all 36 emitted-C tests
then passed. The public probe matched at 1, 2 and 8 realms both normally and with every-allocation
GC stress. In stress mode candidate collection causes the expected refusal and ticket rollback.
Six focused object, array and dictionary native parity probes matched in both modes. Numeric
element side buffers, standalone large blocks, interpreter direct-factory parity and the final
program gates remain open.

### 2026-09-29 -- direct candidate dictionaries carry their property buffer

An ordinary dictionary created inside `isolate(factory)` now gives its separately allocated
property records a candidate-only owner, capacity and generation header. The one normal reserve
path preserves that provenance across growth; the object finalizer frees the header exactly once.
The outgoing certificate validates the header and each property, admits a process-shared key or
an owned candidate key, and records the complete side allocation for incoming pointer checks.
Capsule adoption marks the receiver for spelling-based dictionary lookup, including when a moved
key did not come from its own intern table. Sealed-layout conversion, numeric element side buffers
and standalone large blocks remain separate work.

A native-only public actor probe moved a 40-key static-name dictionary, and the receiver checked
the first key and descriptor, grew the dictionary, and returned the expected 41-key count. The
focused C fixture exercised ordinary versus candidate allocation, resize, a candidate-owned key,
an interior machine alias, a second owner outside the group and provenance tampering. Backend C
tests passed 256 cases with one ignored, and 36 emitted-C snapshots passed with the ABI snapshot
re-recorded. The public probe matched at 1, 2 and 8 realms normally and with every-allocation
GC stress; in stress mode collection invalidated candidate provenance, and the expected proof
refusal rolled the ticket back. Six object/array/dictionary native parity probes matched both
normally and with every-allocation collector stress. These
checks cover this layout and rollback, not the full native-stress corpus or interpreter direct
factory parity.

### 2026-09-29 -- public side-backed array handoff and alias refusal

The native direct-factory actor probe now moves arrays with 128 and 16,384 elements through
`isolate(factory)` and checks their boundary elements in the receiving actor. It also moves a
100,000-element array and has the receiver grow it to index 200,000, checking both retained
elements and the new length. A separate 128-element factory retains a sender alias; the move
refuses with `DataCloneError`, and the sender can still read its array and side buffer. This
exercises the public path for the array element-buffer ownership rule already covered by the C
fixture's resizing, nested buffer, second-owner and provenance checks.

The direct probe completed alone in native release and matched at 1, 2 and 8 simultaneous realms.
With every-allocation GC stress, the 128- and 16,384-element
candidate moves refuse because collection invalidates candidate provenance; retrying their
tickets and the retained-alias case still refuses without consumption or corruption. The stressed
probe also matched at 1, 2 and 8 realms. These checks establish array side-buffer handoff and rollback for this native layout;
they do not establish interpreter direct-move parity or ownership rules for dictionary side
storage and standalone large blocks.

### 2026-09-29 -- direct factory graphs may carry validated temporary blocks

The native candidate certificate now walks the returned root first, then treats every otherwise
unreached candidate block as another walk root. It drains the newly queued edges before accepting
the group. Each temporary must pass the same exact kind, outgoing-edge, side-buffer ownership and
finalizer-layout checks as a returned block; incoming root and outside-edge checks still cover
the whole group. Unknown kinds, edges leaving the group and retained sender aliases refuse.

This admits a `buildGraph` factory with a 13-entry temporary category array and ~1 MB of
returned objects. The native-only public probe now requires the checked transfer. It passed 30
repeated native-profile runs and matched at 1, 2 and 8 simultaneous realms in native release.
A C fixture moved 1,200 shaped objects with distinct strings plus an unreachable, side-backed
pointer-bearing array, then marked and collected the receiver. The live graph remained readable,
the temporary blocks were collected, and the fixture passed with AddressSanitizer. An older
assertion that all unreachable cells must refuse now checks that closed temporary cells succeed
while external edges and unsupported temporary kinds still refuse. The backend crate passed
254 tests with one ignored; all 36 emitted-C tests passed after re-recording the ABI snapshot.
Three prepared-transfer native parity probes matched
under every-allocation collector stress, and the small direct-move rollback probe matched at
1, 2 and 8 stressed realms. The program gates remain due. The
interpreter still refuses direct factory moves; this improvement is native-only and issue 10
remains open for parity, wider layouts and final stress verification.

The seven-round public graph benchmark measured direct construction at 8.31 ms and direct
handoff plus reply at 17.54 ms, against 12.93 ms for a prebuilt-graph clone call and 4.93 ms for
prepared handoff plus reply. Direct proof still scans the sender heap for incoming aliases; the
size-dependent certificate is a measured performance cost. Keep the explicit clone and prepared
paths available while the alias proof is optimized without weakening it (issue 13 has the raw
measurement and differing-construction caveat).

### 2026-09-29 -- ordinary clone control isolates the direct-move GC failure

`probes/actors-clone-realistic-native.mjs` sends the same nested, shared-category graph as the
direct-move regression through an ordinary actor call. Its full 6,000-item graph completed with
the checked sum in native release mode. A 160-item instance with the same edge kinds completed
with `KILN_GC_STRESS=1`; the probe also matched at 1, 2 and 8 simultaneous realms with native
collector stress. This is a control for the public clone path, not proof that the direct-move
receiver lifetime is correct. The strict direct outgoing guard remains in place.

The full `mise run native-stress` was attempted from this worktree. It stopped before native
comparison because the sandbox cannot complete the HTTP listener in
`app-bun-serve-global.mjs`: the interpreter reported an unsettled top-level await. The raw log
is `target/gate-logs/architecture-check/native-stress.log`; the required full gate is still due
in an environment that permits loopback sockets.

A backend C fixture also moves a closed graph of 1,200 shaped objects and distinct inline
strings across multiple candidate chunks, then marks and collects the receiver heap. Every
object and string remains indexed and readable. This checks the receiver's multi-chunk string
tracing for a graph with no unreachable candidate blocks. It narrows the realistic crash to
the newly admitted temporary-block layout or the public handoff path; it does not certify
pointer-bearing orphan blocks for transfer.

### 2026-09-29 -- closed temporary blocks need a receiver-GC proof

An attempted expansion of the outgoing certificate walked every otherwise unreachable
candidate block and accepted it if its layout and edges were individually closed. A small
factory with an orphan array then transferred, and the C closure fixture passed. The realistic
~1 MB `buildGraph` factory also passed the handoff, but its actor crashed while reading the
graph. A symbolized core placed the fault in `kiln_val_to_boundary`, dereferencing an unmapped
string pointer during the actor's sum walk. The same binary returned the correct checksum with
`KILN_GC=0`. This establishes a GC-dependent unsafe acceptance; it does not identify whether
the missed edge is in receiver rooting, tracing or transferred side ownership. The outgoing
expansion was reverted, the release CLI and probe were rebuilt, and the realistic factory again
refuses with `DataCloneError`. `probes/actors-direct-realistic-native.mjs` now permits either a
correct checked transfer or a proof refusal, but fails on a crash; it matched at 1, 2 and 8
realms. Do not admit pointer-bearing temporary blocks merely because the outgoing walker can
follow them. First prove their lifetime through receiver collection with a focused fixture and
every-allocation stress.

### 2026-09-29 -- direct candidate moves a multi-chunk graph

The native-only `probes/actors-direct-multichunk-native.mjs` builds 5,000 ordinary objects
inside one `isolate(factory)` scope, returns their array without retaining an alias, and checks
the receiver's length and selected values. Its block headers and object payloads exceed a
single 256 KiB nursery chunk. The public actor move succeeded alone and matched at 1, 2 and 8
simultaneous realms. The smaller direct probe owns the collector-stress rollback check: forcing
collection during this large factory invalidates candidate provenance, so this positive case
does not claim to succeed under every-allocation stress.

### 2026-09-29 -- candidate-sized nursery blocks keep direct strings movable

During a direct `isolate(factory)` scope, a block larger than a quarter nursery chunk previously
went to the standalone allocator even when it fit a fresh 256 KiB chunk. That broke outgoing
closure for a factory returning `{text: prefix + prefix}` with an 80 KiB result string. The
nursery's three allocation entry paths now use the full usable chunk size only while a live
actor candidate is being built. Ordinary small allocations return before checking candidate
state, and collection restores the ordinary quarter-chunk threshold. The native-only public
probe checks the 80 KiB answer's length and boundary bytes along with a small direct move and
a retained-alias refusal. Backend C tests passed 253 unit and 36 emitted-C cases; the probe
matched at 1, 2 and 8 realms both normally and with every-allocation collector stress. In
stress mode collection invalidates the candidate, so the proof refusal and ticket rollback are
the expected outcome. Three prepared-transfer interpreter/native probes (ordinary, large-string
and sparse-array) also matched under every-allocation native collector stress. A factory calling
`String.prototype.repeat` still refuses even for a small final string: its intermediate
allocations are not a closed movable candidate. Direct factory transfer remains restricted by
the complete outgoing and incoming proofs.

### 2026-09-29 -- native direct-move success and rollback exercised end to end

A native-only public actor probe now sends a simple `isolate(factory)` object with no retained
alias, checks the receiver's value, then keeps a sender alias to a second object and requires
`DataCloneError`. The eligible move succeeded in 20 ordinary native-release runs. It also
matched at 1, 2 and 8 simultaneous realms. With `KILN_GC_STRESS=1`, every-allocation
collection invalidated the candidate, so the first transfer refused; retrying the same ticket
also refused with the proof error rather than reporting a consumed ticket. The retained-alias
case still refused, and the stressed probe matched at 1, 2 and 8 realms. The probe lives at
`probes/actors-direct-transfer-native.mjs`, outside the interpreter/native parity corpus because
the interpreter currently has no direct-move proof and refuses this case. This demonstrates one
positive native layout and safe rollback; broader graph kinds, interpreter parity and the final
native-stress gate remain open.

### 2026-09-29 -- prepared arrays preserve holes

`prepareTransfer` now accepts ordinary sparse arrays in both engines. The plain-graph encoder
records absent indices, including a trailing gap after the last stored element; the native
private arena preserves hole tags and its ownership walk treats them as absent edges. Actor
message copying also retains array length and checks own indices so inherited numeric values
cannot fill a hole. A public probe covers a 130-element backing array, a trailing hole,
snapshot isolation, and an ordinary sparse actor message. The three touched crate test suites,
focused native and interpreter runs, and a focused native collector-stress run passed. Direct
factory isolation and other unsupported graph kinds remain separate work under this issue.

### 2026-09-29 -- 100 MB buffer transfer has constant observed handoff cost

The cross-realm benchmark now transfers one `ArrayBuffer` out and back at 1 KiB and 100 MiB,
checks sender detachment and boundary bytes, and touches no interior payload bytes inside the
clock. Seven interleaved native-release rounds with matching checksums measured kiln medians
of **0.0802 ms and 0.0800 ms**, respectively (ranges 0.0795–0.0808 and 0.0794–0.0812 ms).
On the same host Node measured 0.0170/1.6157 ms and Bun 0.0177/0.5321 ms. The observed kiln
handoff meets this issue's size-independence target for these sizes; it does not prove a
constant bound for every buffer size or graph kind. The quick `--only messages` benchmark
rendered both new rows with no divergences. Raw records and its report are under
`~/.cache/kiln/buffer-transfer-size-2026-09-29/`. General direct graph moves and the final
collector-stress gate remain open.

### 2026-09-29 -- prepared snapshots carry standalone large strings

The private arena now records a string larger than the nursery's quarter-chunk limit as a
standalone GC block in the same ownership group. Its manifest, temporary closure proof and
receiver preflight include that block; receiver adoption attaches it to the large-object index
and GC list before publishing the inbox root. Discard frees it with the arena. A C fixture
checks abandonment, sender collection, receiver adoption and receiver collection; the public
actor probe carries an 80 KiB string in a prepared graph in both engines and under native GC
stress. This broadens the prepared-snapshot path without claiming direct zero-copy factory
isolation or support for other standalone block kinds.

### 2026-09-28 -- actor graph status wakes its sender

The graph coordinator now queues an advisory wake on the authenticated sender reply endpoint
after it publishes a terminal graph status. The sender checks `GRAPH_STATUS` on that wake and
consumes or restores its ticket from the host result; a numeric message cannot authorize a
move. The actor's receiver already used the same PortWait pattern. Normal graph calls no longer
schedule repeated `setTimeout(0)` status polls. A timer remains only when the reply endpoint
has closed before it can receive its wake, or a status operation throws while recovering.

A public probe disables `setTimeout` during eight successful prepared calls and a refused
direct move with a retained sender alias. Both engines pass. Native collector stress matches
the probe at 1, 2 and 8 simultaneous realms, alongside the existing prepared-transfer probe.
The builtins and provider crates passed 107 and 57 tests.

### 2026-09-28 -- dedicated and mixed claims wait on live requests

The follow-up applies the cancellation-based wait to dedicated and mixed
scheduled/dedicated graph pairs. A busy claim waits in numeric realm order while its request
remains live; a closed realm or cancelled request releases any partial claim. The mixed
rendezvous waits for both owner callbacks rather than using a one-second scheduling deadline.
A scheduler test covers a queued live claim and cancellation while another claim holds the
realm. The two prepared-transfer probes matched alone and at 1 and 2 dedicated realms under
collector stress; the older probe also matched at 2 and 8 scheduled realms. The touched
runtime/provider crates passed 58 and 211 tests.

### 2026-09-28 -- graph controls wait for scheduler progress

Two top-level realms running prepared actor transfers under collector stress exposed an
unrelated scheduling refusal: the deterministic scheduler's one lane could run another realm's
long stressed turn while the first transfer's paused control waited behind it. A fixed one-second
control deadline reported `graph transfer needs ownership proof`, although both realm claims had
already reached their boundaries. The same failure reproduced on the older prepared-transfer
probe, before computed dictionary keys were involved.

Scheduled graph claims and queued controls now wait while the request is live. Cancellation can
remove a control before it starts; a started control finishes before its claim is released.
The two-sided rendezvous likewise waits for both callbacks or a real cancellation/failure,
and after publication begins both callbacks finish without a timeout. The two prepared-transfer
probes matched alone and at 1, 2 and 8 simultaneous realms under every-allocation collector
stress. The runtime and provider crates passed 57 and 211 tests respectively.

### 2026-09-28 -- prepared graphs carry computed string keys

The private-arena clone now accepts ordinary objects with realm-local shaped or dictionary
keys. It copies keys and dictionary records into the movable chunks, certifies each key as an
outgoing graph edge, and keeps the records in the object block so no separate property buffer
needs ownership at handoff. After adoption, a dictionary read compares the canonical pointer
and then the spelling; a capsule bit tells the receiver that an intern-table miss cannot prove
absence in its prototype walk. Ordinary realm-local and shared shapes retain their fast path.
Adding a property after transfer moves an in-block dictionary to ordinary malloc storage while
keeping its key bytes rooted. Null-prototype source records follow structured-clone semantics
and become ordinary receiver objects; the interpreter's plain-graph inspector now accepts them.

The new public probe checks computed shaped and null-prototype keys, source mutation after
preparation, receiver reads, descriptors and growth. The focused binary passed under
`KILN_GC_STRESS=1` and `KILN_KEYS_CHECK=1`. Full collector stress passed 308 matches, one
known refusal and zero divergences with 40 declared skips. Standalone large GC blocks,
direct-candidate dictionary side storage, and broader direct `isolate(factory)` moves remain.

### 2026-09-28 -- prepared snapshots span multiple nursery chunks

The public `prepareTransfer` path capped its private arena at one nursery chunk, so it refused
the same 1 MB plain graph used by the concurrency benchmark even though its capsule proof and
receiver transaction already support multiple chunks. The actor operation now permits up to
64 MiB of chunks per prepared snapshot, allocated on demand. The cap is separate from the
parallel worker arena budget. A 2,000-object public actor probe checks a snapshot spanning
multiple chunks, source mutation after preparation and receiver contents. Its smaller companion
probes cover the same allocation and handoff under every-allocation collector stress.

Seven runs of the same native-release public actor benchmark, ten paired calls per run, kept
matching graph checksums: ordinary structured-clone call median 75.67 ms; preparing a private
snapshot 4.79 ms; prepared handoff and reply 4.63 ms; prepared total 9.39 ms. The focused
`--quick --only messages` report also rendered all four rows without a divergence. Backend-C
tests passed 253 unit cases and 36 emitted-C cases; native parity reported 346 matches, one
known refusal and zero divergences. Full native collector stress reported 306 matches, one
known refusal and zero divergences with 40 declared skips; the large probe is skipped there
because it builds 2,000 objects. Both prepared probes matched at 1, 2 and 8 concurrent realms.
Raw measurements and logs are under `~/.cache/kiln/actor-graph-2026-09-28/`.

This extends the existing supported plain-graph snapshot path. Dynamic dictionary keys,
standalone large blocks and direct `isolate(factory)` parity remain open.

### 2026-09-28 -- public prepared snapshot transfer

`prepareTransfer(value)` now returns an opaque handle for supported ordinary object and
dense-array graphs. Native preparation copies into a private arena and transfers its chunks
through the pair-claimed actor transaction. The interpreter retains a private snapshot and
clones it into the accepted actor message, so its observable snapshot and single-use ticket
behavior match. The source stays usable; unsupported graphs refuse with `DataCloneError`.
The public probe covers source mutation, ticket reuse and refusal, cycles, a side-backed
array, and both engines. Native GC stress and 1/2/8-realm runs passed. Dynamic property keys,
standalone large blocks, and direct `isolate(factory)` parity in the interpreter remain open.
The interpreter's plain-graph inspector checks current array holes rather than its historical
`sparse` flag: `Array(128).fill(7)` is dense even though it began with holes.

### 2026-09-28 -- engine operation for prepared handles

The native worker operation now creates a prepared private arena from a plain-data value and
registers it under a weak opaque JS handle. The existing isolated-handle validity check also
recognizes a live prepared arena. A focused executable fixture checks that the snapshot root
is outside the sender heap and releasing the handle discards it. The operation remains private
until `prepareTransfer` and matching interpreter behavior are implemented; it does not change
the accepted public graph-transfer surface.
The backend suite passed 253 tests (one ignored) and 36 emitted-C tests; native parity
reported 343 matches, one known refusal and no divergences.

### 2026-09-28 -- prepared arenas use the claimed actor handoff

The existing pair-claimed graph transaction now accepts a prepared private arena as a second
source type. The sender stages its certified capsule without scanning or unlinking the sender
heap; receiver preflight still reserves capacity first. Cancellation before adoption retains
the arena and restores the request for reuse. After receiver adoption publishes the inbox root,
sender commit releases the weak handle's arena metadata without freeing the adopted chunks or
array side buffers. A focused paused-control fixture cancels, retries, adopts and consumes one
prepared graph, while the original direct-candidate control test still passes. The public
`prepareTransfer` operation, interpreter parity, dynamic keys and large blocks remain open.

### 2026-09-28 -- native prepared-handle lifetime follows GC reachability

The native heap now holds weak native-resource entries keyed by exact local JS object handles.
After the ephemeron mark reaches a fixed point, a dead handle releases its resource before its
block is swept; a failed mark leaves resources intact. Explicit consumption and realm teardown
use the same disposal callback. The prepared-graph arena is registered through this path, so its
chunks and side buffers no longer need a process-lifetime registry or a strong sender-heap root.
Focused C fixtures exercise a live rooted handle, an abandoned handle, explicit release, the
teardown disposal routine, and a real multi-chunk prepared graph retained and then discarded
through GC. This is still internal: the actor pair transaction and interpreter equivalent must
land before `prepareTransfer` is exported. The backend suite passed 251 tests (one ignored) and 36 emitted-C
tests. Native parity reported 343 matches, one known refusal and zero divergences; collector
stress reported 304 matches, one known refusal, zero divergences and 39 declared skips. Logs are
under `~/.cache/kiln/clone-wide-2026-09-28/arena-weak-*`.

### 2026-09-28 -- an owned arena operation with discard and receiver adoption

The off-heap clone's test-only sequence is now a runtime `clone_arena_prepare` /
`clone_arena_adopt` operation. Preparation builds in private nursery-format chunks, uses a
temporary index of only those chunks for the existing outgoing-closure certificate, and then
discards the index. The arena retains its own chunks and side-buffer manifest until explicit
discard or receiver adoption. Receiver preflight precedes the ownership change; failed preflight
leaves the arena available for a later attempt. Sender GC between preparation and adoption is
safe because its heap never indexes the copied graph. A focused C fixture exercises failed
cloning, sender collection, failed then successful receiver preflight, and discard of a
multi-chunk graph with a side buffer. This is still an internal operation: an actor-facing opaque
handle, handle lifetime cleanup, claimed-turn transaction, interpreter parity, dynamic keys and
large blocks remain open. Full backend and native evidence is recorded with the landing commit.
The backend suite passed 250 tests (one ignored) and 36 emitted-C tests; the final focused
preflight fixture passed. Native parity reported 343 matches, one known refusal and no
divergences. Collector stress reported 304 matches, one known refusal, no divergences and 39
declared skips. Logs are under `~/.cache/kiln/clone-wide-2026-09-28/arena-owned-*`.

### 2026-09-28 -- build and hand off a graph outside the sender heap

The native plain-data clone now selects an off-heap arena allocation policy for shared-shaped
objects, string values and arrays. It uses the existing nursery-format worker chunks, allocates
external element buffers for large arrays, and records every side buffer for rollback. The
ordinary and arena allocators share object and array initializers so their fields cannot drift.
A failed clone releases its arena chunks and side buffers without changing the sender heap.

An executable C fixture builds a cyclic 12-slot object with a 128-element side-backed array,
forces sender collection, and confirms that the copied graph remains outside the sender heap.
It attaches the arena chunks to a temporary heap view, runs the existing outgoing capsule proof,
preflights the receiver, detaches and adopts the chunks, then checks publication of the receiver
root. That route never scans the sender heap for incoming aliases. It is still an internal route:
the actor-facing opaque handle, cleanup when abandoned, claimed-turn transaction and matching
interpreter behavior remain to implement. Dictionary keys and standalone large blocks also remain
outside the accepted subset. The backend unit suite passed 250 tests (one ignored), and the 36
emitted-C tests passed after updating the ABI snapshot and inspected prelude-size ceiling. Native
parity reported 343 matches, one known refusal and no divergences; full collector stress reported
304 matches, one known refusal, no divergences and 39 declared skips. Logs are under
`~/.cache/kiln/clone-wide-2026-09-28/`.

### 2026-09-28 -- candidate clone owns string values in a closed graph

The native plain-data clone walk has an internal candidate mode that copies Latin-1, UTF-16 and
empty string values instead of retaining sender string pointers. Its identity table preserves
repeated references; object and array copying keeps cycles. A C fixture clones a 12-slot object
that points to itself and to a 128-element array with a side buffer, then proves the complete
candidate passes the outgoing capsule closure check. The source graph stays intact. Ordinary
`structuredClone` retains its prior mode and behavior. The backend suite passed 249 tests (one
ignored) and 36 emitted-C tests after regenerating its ABI snapshot; native parity passed 343
matches, one known refusal and zero divergences. Full native collector stress reported 304
matches, one known refusal, zero divergences and 39 declared skips. Logs are under
`~/.cache/kiln/clone-wide-2026-09-28/`.

This is an allocation-policy seam, not a private transfer arena or public `prepareTransfer`.
The candidate remains attached to the sender nursery and the current handoff still scans the
sender heap. The separate arena must own blocks and side buffers, survive a forced collection
during preparation, and remap dynamic keys to the receiver before publication. See the
[allocation-group design](../graph-transfer-allocation-groups.md).

### 2026-09-28 -- separate prepared snapshots from direct ownership moves

The architecture review in [the allocation-group design](../graph-transfer-allocation-groups.md)
now names `prepareTransfer(value)` as a proposed explicit preparation operation. It copies a
supported graph into a private arena, leaves the input and its aliases usable, and returns only
an opaque handle to the snapshot. The existing `transfer(handle)` then moves that arena at
message acceptance. `isolate(factory)` keeps its direct, no-copy ownership contract and still
fails when its certificate cannot prove isolation. A prepared arena removes the sender-heap
incoming scan from the general path by construction; preparation itself still costs one graph
walk and copy. ADR 0072 records this as an issue-10 addition pending implementation and matching
behavior in both engines.

As a small native preparation seam, plain-data cloning now sizes a destination object's slots
before allocating it when its final shape is known. An executable C fixture copies a 12-slot
object into candidate storage and proves the resulting copy passes the graph capsule's ownership
certificate. This does not implement the private arena or extend the public actor API. The
backend crate passed 248 tests (one ignored) and 36 emitted-C tests; ordinary native parity
reported 343 matches, one known refusal and zero divergences. Full collector stress reported
304 matches, one known refusal, zero divergences and 39 declared skips. Logs are under
`~/.cache/kiln/clone-wide-2026-09-28/`.

### 2026-09-28 -- sort side spans once for multi-array graph proofs

The first side-buffer certificate compared each new allocation with every earlier span and
searched all spans for each incoming pointer. It now sorts the completed certificate by address,
checks adjacent spans for overlap, and uses binary search for incoming aliases. For `S` owned
side allocations and `P` incoming pointer checks, those parts change from O(S² + P·S) to
O(S·log S + P·log S). The sender-heap and machine-root scans still run; this is a scaling fix
within the strict candidate path, not a replacement for the private-arena design.

A focused C fixture certifies two side buffers, their sorted spans and raw aliases to each. The
public native probe transfers a nested pair of side-backed arrays, and a separate
`probe-side-buffer-graph-scale.mjs` transfers one graph with 1,025 side-backed arrays. The
16-realm public probe produced identical output and zero failed statuses. The backend crate
passed 247 unit tests (one ignored) and 36 emitted-C tests; the ABI snapshot was regenerated
with `INSTA_UPDATE=always`. Ordinary native parity reported 343 matches, one known refusal and
zero divergences. Full native collector stress reported 304 matches, one known refusal, zero
divergences and 39 declared skips. Logs are under
`~/.cache/kiln/graph-side-sorted-2026-09-28/`.

### 2026-09-28 -- move candidate arrays with owned external element buffers

The capsule now includes a candidate array's provenance-backed external element buffer in its
outgoing ownership certificate. It checks the candidate ID, exact array owner, capacity,
generation, byte extent and exclusive side span; a side-backed orphan or overlapping owner
refuses. During the exclusive incoming proof, the same span is temporarily visible to C roots,
conservative machine words, provider roots and outside-heap edge scans, then removed before the
sender resumes. An unrelated array pointing at the buffer is an incoming alias and refuses.
After adoption the receiver can grow the array through `realloc`, and its collector finalizer
frees the header and buffer once.

The focused backend fixture passed, including injected outside ownership and a corrupt
candidate ID. The full backend crate passed 247 unit tests (one ignored) and 36 emitted-C tests;
the ABI snapshot was regenerated with `INSTA_UPDATE=always`. A public native actor probe moved
128-element and 100,000-element arrays, grew the latter in the receiving actor, refused a
retained sender alias to a side-backed array, and matched in 16 simultaneous realms with zero
failed statuses. Ordinary native parity reported 343 matches, one known refusal and zero
divergences. Full native collector stress reported 304 matches, one known refusal, zero
divergences and 39 declared skips. Logs and realm outputs are under
`~/.cache/kiln/graph-side-2026-09-28/`. Dictionary and shaped-object side storage, large GC
blocks and the general private-arena preparation path remain open.

### 2026-09-28 -- architecture review after candidate side-buffer provenance

The current candidate capsule is a correct narrow zero-copy path, but its sender-heap and
conservative machine/root scans make storage-kind-by-storage-kind expansion a poor default route
to general mutable graph messaging. The [allocation-group design](../graph-transfer-allocation-groups.md)
now distinguishes that strict path from a proposed private transfer arena: snapshot a supported
graph into arena-owned blocks and side buffers, expose only an opaque handle to the snapshot, then
move that arena under the existing atomic handoff. The preparation copy is explicit and measured;
handoff does not copy payloads. A direct-build fast path may skip the preparation copy only with
the same no-alias proof. No implementation or user-facing contract changed in this review, and
issue 10 remains open. Prioritize actor startup and scheduler acceptance before expanding the
strict capsule's accepted C layouts.

### 2026-09-28 -- candidate array side-buffer provenance

Candidate arrays now place a small ownership header before an external element buffer. The
header records the candidate ID, array owner, capacity and allocation generation. Array growth
updates the same header through the centralized reserve path; the array finalizer frees its
allocation base, including after the candidate scope ends or a realm adopts the array's block.
Ordinary arrays keep their existing buffer layout and allocator path. A focused C fixture checks
ordinary and candidate arrays, two growths, the element values and finalizer teardown.
The candidate check returns before a block-table lookup on ordinary array growth.
The backend crate passed 247 unit tests and its 36 emitted-C tests after re-recording the ABI
snapshot with `INSTA_UPDATE=always`. A fresh release CLI passed ordinary native parity with
343 matches, one known refusal and zero divergences; full collector stress passed with 304
matches, one known refusal, zero divergences and 39 declared skips. Logs are under
`~/.cache/kiln/graph-date-2026-09-28/side-header-*`.

This is provenance only. The capsule still refuses side-backed arrays until its outgoing walk,
machine-word scan and outside-heap incoming scan include the owned side span. A public
128-element transfer must pass those checks before it is enabled.

### 2026-09-28 -- allocation-group design for general graphs

The current in-block capsule is a subset, not the final representation. An attempted
candidate-only expansion of array in-block storage above 64 values was discarded before
landing: it would still refuse larger arrays, dictionary objects and external side buffers.
[The allocation-group design](../graph-transfer-allocation-groups.md) instead treats a graph's
dedicated GC chunks and uniquely owned side allocations as one move set. Its certificate must
include side-buffer contents and spans in both outgoing and incoming alias proofs. This keeps
ordinary array allocation and property-read paths unchanged while extending the move protocol
under ADR 0070. The next implementation step is a provenance-backed array element-buffer
certificate, followed by dictionary storage and standalone large blocks.
The two direct array growth branches in indexed assignment and `length` writes were folded
into `kiln_c99_array_reserve`, so an allocation record can be attached at one seam. The
existing `inline-array-growth.js` and `array-insert-gc.js` probes cover those write paths and
collection during later array insertions.
The backend crate passed 246 unit tests (one ignored) and 36 emitted-C tests. A current release
CLI passed ordinary native parity with 343 matches, one known refusal and zero divergences;
full collector stress passed with 304 matches, one known refusal, zero divergences and 39
declared skips. The logs are under `~/.cache/kiln/graph-date-2026-09-28/array-reserve-*`.

### 2026-09-28 -- Date payloads in native graph capsules

The outgoing capsule proof now admits a Date's exact pointer-free `double` payload, both as a
root and as a value inside an ordinary object. It still requires the candidate's full chunk
closure and the separate incoming alias checks before moving ownership. A focused C fixture
checks receiver adoption and GC reachability of a nested Date. The public native actor probe
checks a Date root and an object containing a Date through `transfer(isolate(...))`, including
the timestamp and `Date` identity after the actor's reply. Other object layouts remain
subject to the existing fail-closed proof. The backend crate passed 247 unit tests and 36
emitted-C tests. The public probe passed in 16 simultaneous realms with identical output and
zero exit statuses. Ordinary native parity passed with 343 matches, one known refusal and zero
divergences; full native GC stress passed with 304 matches, one known refusal, zero divergences
and 39 declared stress skips. Logs are under
`~/.cache/kiln/graph-date-2026-09-28/`.

### 2026-09-28 -- move known-size objects with in-block property slots

An object literal with 9–32 known properties now allocates its slot array in the same GC block
as its header, rather than a separate `malloc` buffer. The shape's 32-property bound determines
that range. A shared slot-resize helper copies the in-block slots before any later growth; the
dictionary conversion and GC finalizer free only separately owned slot buffers. The capsule's
outgoing proof admits an exact in-block pointer and capacity, still refusing external slots,
overstated capacity, local shapes and side storage. This extends zero-copy actor handoff to a
12-property object through the public API.

The C capsule fixture covers adoption and both pointer/capacity refusals. The new
`conformance/native/object-tail-slots.js` covers growth, deletion, spread and descriptor
definition after a wide literal is made, and matches the interpreter under ordinary and stressed
native execution. The public actor graph probe passed for 1, 2 and 16 concurrent realms, three
runs each. `KILN_GC_STRESS=1` intentionally invalidates a candidate while its factory runs,
so a positive graph handoff refuses safely in that mode; the existing focused candidate-stress
fixture checks this rule. This is distinct from GC stress on an already constructed object.
The routine gate passed all 10 steps (2,272 crate tests, 342 native matches, zero native
divergences, 97,110 routine Test262 passes with no chunk regressions). Full native GC stress
passed with 303 matches, one known refusal, zero divergences and 39 declared stress skips.
The gate, stress and focused logs are preserved under
`~/.cache/kiln/graph-object-tail-2026-09-28/`.

### 2026-09-28 -- move arrays with in-block element storage

The native capsule now accepts an array whose elements occupy the extra capacity in its own GC
block, the layout `kiln_c99_new_array_with_capacity` uses for 9–64 elements. The certificate
checks that the pointer names exactly that tail and that the declared capacity fits inside the
block; an external element buffer and an overstated capacity still refuse. Receiver finalization
already treats that tail as part of the block, so moving the chunk transfers its ownership.

The C capsule fixture checks the valid tail, both refusals, receiver adoption and GC marking.
The public native actor probe moves a 12-element array. `kiln-backend-c` passed 245 unit tests and
36 emitted-C tests; a direct run passed with native GC stress; the actor probe matched alone and
for 1, 2 and 16 realms in three runs each. `scripts/native.sh` at four jobs reported 341 matched,
one known `ShadowRealm` refusal, zero divergences. A preceding 16-job run saw an intermittent
interpreter worker-exit line omission in `region-builtins-per-realm`; five direct runs per engine
matched before the clean four-job corpus. External element buffers and other side layouts remain
outside the graph mover. The corrected 12-job routine gate passed all 10 steps: 2,272 crate tests
passed, 341 native matches and zero divergences, and 97,110 routine Test262 passes with no chunk
regressions. Full `mise run native-stress` passed with 302 matches, one known refusal, zero
divergences and 39 explicitly skipped stress probes. Gate and stress logs are preserved under
`~/.cache/kiln/graph-inblock-2026-09-28/`.

### 2026-09-28 -- scheduled graph handoff across shared lanes

The native graph probe exposed two scheduler seams. Provider realm IDs can differ from scheduler
ticket IDs, so paused controls now carry the registered `TurnPauseTarget` through the claim rather
than passing a raw ID into the scheduler. When an actor and caller are pinned to the same lane,
the coordinator uses the already proved dedicated-realm stage/reserve/detach/adopt/consume
protocol with both turn tickets claimed. Distinct-lane pairs retain the two-callback handoff.
An ordinary object with `Object.prototype` also passed the existing native layout proof and
retained its prototype and fields after transfer; the positive probe now checks that case.

The positive probe passed ten runs each at 2, 16 and 32 realms on two lanes, and five runs each
at 2 and 16 realms on one lane. Unsupported layouts and retained sender aliases still refuse.

### 2026-09-28 -- multi-chunk native graph handoff

The capsule now certifies and moves all full nursery chunks from one factory candidate as a
single transaction. Receiver capacity is reserved for the complete group before the sender
unlinks any chunk. A provisional collector hold spans `CANDIDATE_END` to `ISOLATE`, where the
exact returned root passes outgoing closure; marked, sealed chunks remain intact until the
handle dies or the later sender-alias check authorizes handoff. The hold does not count toward
new nursery allocation pressure. A focused C fixture covers a two-chunk move and a forced
collection in the registration window. The native-only
`probe-multichunk-graph.mjs` sends 2,000 linked arrays and checks count and sum on the actor.
It passed alone and in 16 concurrent realms. The backend crate passed 245 unit tests and 36
emitted-C tests. The final four-job routine gate passed all 10 steps (2,268 crate tests, 339 native
matches, 97,110 routine Test262 passes, no third-party baseline regression). An earlier eight-job
gate had one intermittent `Text file busy` launch in an unrelated native fixture; that exact test
passed alone before the final gate. Full native GC stress passed 301 matches, one known refusal
and zero divergences in under three minutes; it reported 38 explicit stress skips.
Five full-size probes were classified as stress skips here because collection on every allocation
made a single probe take over 20 minutes. They remain in ordinary parity, and the new bounded
`recursion-and-parallel-fallback-gc.js` checks their recursion, fallback-allocation and clone-root
shapes under stress.
Unsupported layouts and retained sender aliases still refuse. Broader object layouts remain
part of this issue; the name-matched HIR isolate check was removed earlier.

### 2026-09-28 -- preserve the actor reply while a graph publication settles

A 16-realm actor run exposed a reply ordering race: the worker sent `resolve` and then closed
its reply port while the sender's native graph status was still pending. The close event replaced
the earlier successful completion, so a published graph appeared to fail with
`ActorClosedError`. The sender now retains the first terminal reply until the native status is
known; a refused graph reports `DataCloneError` even when its port closes during cleanup.
The native-only positive graph probe passed ten repeats at 16 realms and ten at 32 realms after
this change. This does not expand the graph layouts admitted by the ownership proof.

### 2026-09-28 -- native actor moves an isolated array and object graph

A native-only dedicated-worker probe now transfers `[3, 5, 8]` and a null-prototype
`{ values: [3, 5, 8], answer: 42 }` through `kiln:actors`; the receiver observes the
values, a consumed ticket refuses reuse, and a separately retained sender alias refuses
with `DataCloneError` while an ordinary actor call still succeeds afterward. Run
`./target/debug/kiln --one-shot --node run --native
docs/issues/realms-and-concurrency/probe-positive-graph.mjs` after building the CLI.

The factory resolves its native worker operation before opening candidate allocation.
Generated call arguments can still leave orphan arrays in the candidate chunk; the
outgoing proof accepts one only when its finalizer has no side ownership and every inline
element is an immediate value. A focused C fixture refuses both a pointer element and
side storage. The incoming proof now enumerates audited outside heap kinds and the
realm-local shape side indexes, including their keys and transitions. Unknown layouts
still refuse. This is a narrow positive subset: unsupported side storage and graphs with sender aliases remain
fail closed. The native-only probe is deliberately outside interpreter parity because
the interpreter does not transfer heap chunks.

### 2026-09-28 -- positive native actor probe remains fail closed

The host now authenticates the sender's reply endpoint as the live peer of the receiver's
transferred endpoint before claiming a graph reservation. The focused host test accepts that
pair and rejects the transferred endpoint itself and an unrelated live channel. A native-only
`transfer(isolate(() => [3, 5, 8]))` probe therefore reaches the paused C ownership check.

The check correctly refuses. Generated call scaffolding allocates two argument arrays inside
the candidate chunk after the factory's result. A temporary narrow proof accepted those arrays
only when all elements were immediate values, their storage was inline, and their finalizer
was inert; focused C checks rejected a pointer element and side storage. With that temporary
proof, the next refusal was `KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED`: an outside sender heap
kind still lacks a complete read-only edge/side-owner audit. The temporary proof and diagnostics
were reverted. Public graph tickets continue to restore on refusal; no object/array handoff is
claimed until the outside-kind audit, whole-chunk proof, and native inbox publication pass end
to end. ArrayBuffer transfer and ordinary actor clone messaging remain available.

### 2026-09-28 -- sender waits for a terminal handoff result

An actor call with a graph ticket now keeps the ticket suspended while native request status is
pending. Status 4 is the only success signal: after the receiver's native root is published and
sender escrow consumed, the sender consumes the ticket and waits for the actor reply. Statuses
1–3 restore the ticket and cancel the held placeholder. Abort or early reply while status is
pending asks native to cancel but defers settlement until one of those terminal states is known;
otherwise a late published move could be incorrectly restored. The actor parity probe inserts
one extra pending poll before its existing refusal check. A native-only positive graph probe and
post-publication abort test are still needed before public admission is considered complete.

### 2026-09-28 -- C owner-lane capsule controls and rollback

The C paused callback now implements numeric commands 3–9 for sender certificate/stage,
receiver preflight, sender unlink, receiver adoption, sender escrow commit, and both pre-unlink
rollback steps. A sender-owned capsule allocation survives the provider registry's exact receiver
take; after adoption, sender commit consumes the escrow and frees that allocation. Stage failure
restores the graph and frees the capsule. Receiver preflight reserves the native inbox and heap
region capacity before unlink, and rollback leaves no inbox root. Any invariant failure after
unlink aborts rather than resuming either mutator with split ownership. The focused C fixture
exercises rollback and retry, then the positive sequence and exact receiver root/consumed sender
entry. Its certificate runs on a clean migrated owner lane: the conservative scan correctly
refuses old candidate pointer bytes left on a synthetic factory's C stack. Public graph tickets
still refuse until the paired post-unlink callbacks and actor result/acknowledgement are verified
end to end.

### 2026-09-28 -- pair coordinator orders the host lease and native controls

Once both scheduled or both dedicated realms acknowledge a claim, the coordinator now acquires
the authenticated host publication lease and orders sender stage, receiver preflight, sender
detach, receiver adoption, and sender escrow commit. It rolls back the staged capsule and native
reservation before unlink on any refusal; a failed control after unlink aborts the process
instead of resuming with a stranded graph. Only after those controls succeed does it publish the
host grant and report request status 4. Two dedicated owner threads execute the sequence directly.
Scheduled and mixed pairs now queue two control callbacks on distinct owner lanes and wait for
both to enter before either stages the capsule. Those callbacks perform the full stage, preflight,
detach, adopt and sender commit through an ID-keyed Rust barrier, so no scheduler command remains
to time out after unlink. A pair pinned to one lane refuses before C mutation. The focused barrier
test proves both callbacks enter before the 3→4→5→6→7 sequence. The C positive controls and actor
acknowledgement are still being implemented.


### 2026-09-28 -- dedicated worker request reaches both turn boundaries

Dedicated realms now register a pause gate at creation, including the initial generated module
entry before the continuous drive starts. A process-lifetime control thread accepts opaque graph
request IDs when no migrating-run coordinator owns the sender. It claims both dedicated realm
IDs in order, wakes parked drives, runs the receiver reserve/rollback and sender negative probe
on their owner threads, then releases both. A focused provider test submits from the initial
entry and observes refusal *after* both realms paused. Cancellation cannot be overwritten by a
late refusal. This is still refusal-only: the host publication lease and positive C transaction
must be connected before a graph ticket can be consumed.

### 2026-09-28 -- sender escrow commit after native publication

An internal allocation-free commit now removes exactly the sender's escrowed weak-map entry
after the capsule has been adopted and its root published in the receiver's native inbox. It
removes the entry from the collection index, clears the entry and the sender's request/escrow
roots, and leaves the original handle unable to retrieve the graph. A wrong request, root or
entry is a process-fatal post-unlink invariant violation. The focused C capsule fixture checks
the native root and sender entry on both sides of this commit. The coordinator must synchronize
the receiver callback's completion before invoking it on the sender lane, while both realms stay
paused. There is still no actor call path to this commit.

### 2026-09-28 -- internal inline object and array capsule subset

The C candidate walk and one-chunk capsule now accept ordinary object/array roots under a narrow
storage contract. An object must use a process-shared plain shape, at most eight inline slots,
no list/sealed/dense side storage, no brand or realm-local layout token, and a null or undefined
prototype. An array may hold at most eight inline elements and no named properties or RegExp
match state. Every value edge must reach a block in the same dedicated candidate chunk; all
blocks there must be reachable. Their finalizer bits remain set for receiver GC, but their
finalizers own no external allocation under these checks. All other finalizable kinds or side
layouts refuse. The focused C fixture moves an object → array → string chain, verifies receiver
GC reachability, and checks refusals for an external slot array, named array properties, local
shape, and missing finalizer bit. A second fixture runs the ordinary object/array constructors
inside the actor candidate scope and verifies their blocks form a preparable candidate graph.

This is still an internal storage/proof subset. Actor tickets stay refusal-only until the pair
coordinator publishes into both native and JavaScript inboxes atomically. The default dedicated
worker mode also needs a turn-boundary pause/control path; success only in the opt-in scheduled
mode would not complete this issue.

### 2026-09-28 -- shared-shape objects in the incoming-edge scan

The read-only sender-heap scan now enumerates the current keys and value slots of an outside
object with a process-shared shape, alongside its list properties, dense elements and prototype.
It detects a candidate address in either a shaped key or slot. An object with a realm-local shape,
a malformed slot count, or a shape mixed with sealed storage still refuses the certificate. The
focused C fixture checks both aliases, the clear path, and those refusals. This only removes a
false incomplete result from the *incoming* scan; candidate object/array finalizers, side
allocations, host indices, and pair publication still require a complete ownership audit. Public
graph tickets remain refusal-only.

After sender unlink, a failed callback or control dispatch must not allow either realm to resume
with a capsule stranded in the provider registry. The coordinator must arrange receiver control
before unlink and make adoption and host lease completion infallible. An unexpected failure after
unlink is a process-fatal invariant violation. The same-lane path needs a single callback that
switches realm context serially rather than waiting for a second callback on the same lane.

### 2026-09-28 -- provider-owned opaque capsule registry

The provider archive now owns a process-wide registry keyed by the host's one-use graph
reservation. It stores only capsule pointer bits plus the sender realm, receiver realm and
sender request IDs; Rust never dereferences the C capsule. Staging checks active sender TLS,
refuses duplicate reservations, requests or pointer bits, and reserves the registry bucket
before any chunk unlink. The sender can mark its exact staged record detached without an
allocation. A receiver take requires its own active realm TLS, the exact sender/request pair
and the detached state, then removes the record without allocation. Pre-detach cancellation
removes the record while C still owns its capsule allocation. Focused provider tests cover
the state gate, wrong identities, replay, cancellation and actual realm TLS entry; the
provider ABI and source digest include the four private exports.

No control callback calls this registry yet. The coordinator must acquire the host publication
lease, hold both realms, stage only after a positive sender certificate, and ensure a receiver
adoption callback is already executing before unlink. A later queued callback can time out.
The same-lane case needs one callback that switches active realm context serially; waiting
for a second callback on that lane would deadlock. Ordinary object and array graph kinds
still lack exact side-allocation and finalizer ownership checks, so public tickets refuse.

### 2026-09-28 -- narrow positive sender certificate remains internal

The sender can now distinguish a fully enumerated clear result from the old negative probe.
At a fresh machine boundary, an opaque request exchanges its registry value into one escrow
slot, checks exact candidate closure, C roots, outside heap edges, provider roots, older stack
words and saved registers, and retains escrow only if all checks are clear. An internal capsule
preparation accepts only a single sealed candidate chunk rooted at an exact cell payload or
inline string data pointer; any failure restores the sender's registry value. The isolated
positive C fixture checks retained escrow and allocation-free rollback. The older-frame
alias fixture still refuses. Collection entries, including weak and dead slots, are now
enumerated as outside edges because the isolated-handle registry itself is a collection.

The machine snapshot is zeroed before `setjmp`: padding and unsaved mask bytes otherwise
contained stale graph addresses and caused a false machine alias. The actual saved registers
are still scanned. No actor path invokes this certificate. A process-wide opaque capsule
registry, receiver preflight under the same pair claim, host publication lease, sender escrow
consumption and receiver native/JS publication remain required before graph tickets can move.

### 2026-09-28 -- sender posts a held inbox marker before the graph request

For a locally issued graph ticket, the sender now posts exactly one clone-safe marker under
the receiver's host-minted grant before asking the pair coordinator to pause the realms.
It suspends the ticket's revocable handle slot while the native request is pending and
restores it after the current refusal. Aborting or rejecting the call cancels the pending
request and the receiver's held marker. A forged `ticketMark` property carrying a real
ticket entry cannot enter this path: positive graph preparation requires the private
module `WeakMap` identity. Cross-plan graph tickets still refuse until the engine owns
ticket authentication. The focused actor fixture covers that forged marker and confirms
the original ticket remains usable. No graph admission or ticket consumption is enabled.

This establishes the JavaScript ordering needed before a future sender unlink. The C
certificate is still negative-only, the detached capsule has no process-wide ID registry,
and the pair coordinator does not yet acquire the host publication lease or run native
adoption under one claim.

### 2026-09-28 -- authenticated graph request reaches paused control

After a receiver grant, the sender now passes its host-minted graph reservation, call ID and
paired reply endpoint with the opaque native request. The coordinator can therefore run the
receiver's one-callback reserve/rollback check and the sender's negative ownership probe under
the pair claim. The actor still raises `DataCloneError` and restores its C request; no graph
ticket is consumed. A focused actor probe with scheduled workers enabled matches the
interpreter. Receiver cancellation now also releases a held graph-placeholder invoke, which is
required before the sender can safely post that placeholder ahead of a fallible request.


### 2026-09-28 -- detached one-chunk storage capsule

The C prelude now has a narrow detached capsule in `runtime/graph_capsule.c`. Sender preparation
checks exact cell or inline-string root identity and whole-candidate closure, then copies only
numeric chunk requirements for receiver preflight. The receiver reserves its native inbox slot
and region-set capacity on its own lane. Sender detach removes the sealed chunk from only its
heap; receiver adoption inserts it into only its heap and publishes the inbox root without an
allocator or callback between insertion and root storage. A focused C fixture checks the chunk
is in neither heap while detached and only the receiver after adoption, including the receiver
GC mark of its new root.

This is a storage primitive, not actor graph admission. The sender alias certificate still has
no positive result; the capsule is not yet carried through an opaque process registry or called
from the pair coordinator. The two realms must stay paused from preflight through publication,
and the sender escrow must be consumed before either realm resumes. A host publication lease and
the JavaScript inbox take must also be part of the same transaction.


### 2026-09-28 -- lane-local paused graph control remains refusal-only

The generated launcher now registers one process-lifetime C callback for paused graph control;
a second native image cannot replace it with a different callback. A scheduler pair claim runs
that callback on each realm's executing lane, entering the provider realm TLS without advancing
its clock. The host request carries only numeric request and reservation IDs. For a granted actor
call, the receiver callback tests a one-chunk native inbox reservation and rolls it back before
returning. The sender callback scans from a fresh machine-stack boundary and restores its escrow;
its result is a negative witness, never permission to move. The coordinator still publishes only
refusal, including when the control callback is unavailable or times out.

The receiver reserve and rollback are deliberately one operation. A separate cleanup command
could be delayed by unrelated lane work after the pair claim ends, leaving a native slot stranded.
A positive transaction still needs a durable receiver reservation with guaranteed cleanup while
both realms are paused, a detached chunk capsule that never borrows two lane-local heaps at once,
and an infallible native and JavaScript inbox publication path after the ownership certificate.


### 2026-09-28 -- host-owned one-use actor graph reservations

The actor receiver can now mint a random safe-integer reservation ID for a granted call through
worker op 34. The host registry binds it to the authenticated actor inbox, that worker key,
the call ID and the transferred reply endpoint. Call IDs are random too; the receiver rejects
duplicates. A sender graph request that supplies a reservation must match the registered actor
route, call and reply endpoints before the existing two-realm refusal claim is queued. A failed
queue submission unclaims the reservation; closing either endpoint or receiver release removes
it. Ops 35 and 36 expose a tagged, one-use receive/release seam, but receive currently returns
only pending or refused. It cannot hand out a graph root until the paused C transaction installs
one. The old two-argument request remains refusal-only during the actor module transition.

The host fixture covers guessed endpoints, duplicate grants, mismatched call/reply IDs,
double claim, unclaim and endpoint-close cleanup. Native parity keeps the graph ticket refused.

### 2026-09-28 -- authenticated actor inbox and revocable ticket root

The actor inbox now uses the worker's own `parentPort`, whose receiving endpoint is recorded
in the host's `WorkerContext` at spawn. Binding requires that exact endpoint, the actor worker
entry, and the current host worker key; another realm cannot claim a guessed endpoint. Once
bound, only that worker key can drain the inbox. The actor route still carries only the
opposite sender endpoint. A focused host test checks wrong-endpoint and outside-worker
refusal, and native actor parity exercises the shared control/mailbox channel.

The ticket marker keeps a frozen entry with a revocable value slot. Consuming a ticket clears
the handle or buffer root even though the marker is nonconfigurable. A graph ticket can be
suspended and restored through that entry, but the actor request bridge does not yet suspend
it: the C request must return the exact handle on rollback and the native/JS inbox reservation
must be atomic first. Graph tickets continue to refuse. The host binding secures the receiver
endpoint for future publication; it does not certify sender graph ownership or make the
existing native inbox slot visible to JavaScript.

### 2026-09-28 -- actor grant carries an opaque request to a two-realm claim

When a graph ticket appears in an actor call after mailbox grant, the native path now roots
only its handle in the C pending-request slot and queues its opaque ID with sender and receiver
realm IDs. The worker binds its inbox endpoint to its provider realm ID; the actor route resolves
the receiving endpoint without passing a heap pointer. The migrating launcher receives the
request on its coordinator thread, claims both scheduler targets, waits for both turn-boundary
acknowledgements, then **always refuses** and releases them. A later sender turn polls the result,
rolls the C request back, cancels the mailbox reservation and reports the existing
`DataCloneError`. The ticket stays unconsumed and can be retried. A deterministic provider
fixture checks that both realms paused before refusal. Dedicated actors and unscheduled senders
cannot be claimed and refuse immediately; scheduled workers register a pause target only when
`KILN_WORKER_SCHEDULER` is enabled.

The inbox-endpoint realm binding is currently a scheduling hint from the actor worker's
internal module, not an ownership certificate for the transferred port. Graph admission remains
disabled. Before any success status, the native and JS inbox slots must be reserved together,
the sender machine and root certificate must run while both realms are paused, port ownership
must be authenticated by the runtime, and publication plus ticket consumption must have an
infallible commit path. The current C incoming probe still always rolls back.
The ticket's nonconfigurable `ticketMark` property currently stores an entry that retains the
handle, in addition to the `WeakMap`; clearing the map alone cannot remove that sender root.
The consuming ticket needs an engine-owned revocable entry with a proof-time root exchange.

### 2026-09-28 -- provider realm IDs resolve to scheduler pair targets

The migrating launcher registers each live provider realm ID with its scheduler pause target
after the generated `identify` callback. The process registry stores only IDs, ticket state and
a weak scheduler reference. It removes the mapping before finish or realm destruction; the
legacy pinned and dedicated-thread runners have no entry and refuse lookup. An external
coordinator can resolve two IDs into a pair claim, rejecting unknown IDs, different schedulers
and busy bodies. A deterministic provider fixture checks the pause, release and teardown.

This supplies an opaque-ID lookup for a future graph request. The actor grant does not yet
submit that request after its generated callback unwinds. The machine-stack certificate and JS
mailbox publication are also missing; graph tickets still fail closed.

### 2026-09-28 -- scheduler pair claim at turn boundaries

The turn scheduler now supports an atomic claim of two live scheduler realm IDs in ascending
order. It wakes both tickets and acknowledges only before a body turn, after any in-flight turn
has returned. A successful external wait holds both bodies paused until its RAII claim is
released; cancellation invalidates the wait and delays body teardown until release. A
deterministic one-lane fixture uses one pinned and one movable body to check the boundary,
concurrent claim refusal, stopped turns, cancellation, and rollback. The scheduler's ordinary
turn path avoids a mutex when no claim exists.

The focused scheduler suite passed 15/15, and `kiln-native-provider-ffi` checked. A full
`kiln-runtime` crate run passed 182 tests and failed 20 socket-listener tests because this
container denied bind/listen with `EACCES` or `EPERM`; those failures did not exercise the
claim path.

No actor path invokes the claim yet. Provider realm handles still need a mapping to scheduler
tickets, and the pending graph request must reach the coordinator after its generated callback
unwinds. The unscheduled drive must refuse or use an equivalent pause. The machine
stack/register proof and JS inbox publication are also missing, so graph tickets still fail
closed.

### 2026-09-28 -- receiver root reservation and publication primitive

The C receiver now reserves one native inbox root slot and its heap region-set capacity before
any sender chunk is unlinked. A reservation records the receiver epoch and region count; a
stale token, collection, or intervening region mutation refuses. Rollback before unlink clears
the slot without allocation. A low-level helper checks a supported root and exact outgoing
closure, relinks a validated candidate batch, and stores the receiver root immediately, with
no fallible operation after unlink. The published slot is marked by the receiver collector and
cannot be rolled back. A focused C fixture covers failed preflight, stale IDs, rollback,
successful root publication, and the collector root.

The native slot is distinct from the actor's JavaScript mailbox reservation. There is no
two-realm scheduler pause, sender stack/register certificate, or JS inbox adoption yet, so no
actor code calls this helper and graph tickets still raise `DataCloneError`. The transaction
document defines ascending realm-handle claim order and turn-boundary acknowledgements needed
before this primitive can participate in a handoff.

### 2026-09-28 -- opaque sender request and exact escrow exclusion foundation

The C runtime now has one pending isolated-handle request per realm, identified by a
monotonically increasing numeric ID. It retains only the handle while pending, rechecks its
weak-key registry entry when escrowing, and restores the graph and clears the request without
allocation on refusal. An old or mismatched ID cannot escrow or cancel a newer request; any
registry write while a request waits invalidates it. The
C-root scan can exclude only the active request's escrow-value slot; a second slot containing
the same candidate pointer remains an alias. Focused C fixtures cover ID replay, rollback,
the exact exclusion, a duplicate root, and invalidation after a registry write.

This is an internal storage seam, not a scheduled handoff. The actor grant callback still
refuses graph tickets. The scheduler must enqueue the ID, reach a turn boundary with all
generated frames unwound, establish the machine-stack boundary before resolving the ID, and
pause both realms through proof and publication. The current conservative GC stack scan mutates
marks and does not provide that boundary or a read-only register result. No positive ownership
certificate is issued yet.

### 2026-09-28 -- source-level factory chunk attribution

The native actor module now brackets the synchronous `isolate(factory)` call with private
begin/end operations. Begin seals any ordinary nursery fill and tags the fresh chunk with one
candidate ID. Every nursery rollover during the factory keeps that ID; end seals the last
factory chunk and switches ordinary allocation to a distinct chunk. This uses the existing
allocator paths, including the inlined bump and multi-string run, so a candidate chunk cannot
contain a pre-factory or post-factory block. Collection is deferred while the candidate nursery
is active and under a 64 MiB filled-chunk ceiling, then resumes after sealing; crossing that
ceiling or exhausting nursery chunks permits collection during the factory and invalidates
transfer eligibility. GC stress also retains its every-allocation collection cadence during
the factory, invalidating candidate provenance rather than deferring collection. A thrown
factory clears its chunks' candidate IDs, and failed cleanup preserves the factory's original
exception; the
blocks remain in the heap because callbacks may have exposed them. Nested factories work as
JavaScript but get no independent candidate scope. The interpreter returns a no-candidate
answer and keeps its existing private realm table.

The C fixtures prove the chunk boundary, epoch, exact one-block closure, nested refusal,
throw/abort reset, and stress collection during an active candidate. The native actor probe
checks that a cleanup failure cannot mask the factory throw, as well as nested behavior under
GC stress.
This is allocation provenance only. A factory can still allocate tenured or oversized blocks
outside its candidate, make side allocations, or expose candidate objects; the outgoing and
incoming proofs must reject those. The collector may invalidate a candidate at the next
collection after the factory; no graph ticket is admitted, and source-level object/array graphs
remain unsupported by the current closure walker. Each candidate forces a nursery chunk
boundary, so it may retain up to one chunk of unused capacity until collection.

### 2026-09-28 -- engine-owned isolated registry and reversible escrow

`isolate(factory)` now registers its result through the engine's private actor operation. The
TypeScript module no longer has a `WeakMap` containing the graph. The native realm roots a
private weak-key collection: a live opaque handle keeps its graph alive, while an abandoned
handle can be collected with its graph. The interpreter keeps the same handle lookup in private
realm-session state. Ticket and handle properties still reveal no graph value.

The native realm can exchange one registry entry's graph into an explicit escrow root and
restore it to the same entry without allocation. A direct C fixture checks the exchange,
double-begin refusal, rollback and subsequent lookup. This helper has no commit operation or
actor caller yet; it does not establish a safepoint or ownership certificate. Graph tickets
still refuse. The future scheduler request needs an opaque handle ID, a verified stack/register
boundary, complete graph candidates and receiver publication before it can call this helper.

### 2026-09-28 -- actor safepoint transaction and provider completeness refusal

`graph-handoff-transaction.md` records the engine-owned handle escrow, sender turn boundary,
two-realm pause, conservative machine scan, rollback and infallible publication phases against
the current actor and scheduler implementation. It identifies why the JavaScript `WeakMap`
handle, mailbox `grant` callback and current candidate arena cannot yet form a sound transfer.
The actor probe now retries a refused graph ticket and verifies both refusals leave it
unconsumed. No graph ticket is admitted.

The incoming probe previously discarded a nonzero result from the provider root visitor. It
now distinguishes `KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED` from a found provider-root alias;
a mark-only registered callback produces the incomplete result in the focused C fixture. This
still has no accepting certificate. Candidate allocation from `isolate`, exact escrow-slot
exclusion, sound stack/register capture, and atomic destination publication remain blockers.

### 2026-09-28 -- read-only machine-word classifier; stack boundary still unproved

The collector's conservative scan (`kiln_c99_gc_scan_stack`) is not an ownership proof. It
mutates mark bits, runs only when `mid_turn` is true, and starts in its own frame. The incoming
probe runs at no established actor safepoint and already holds the candidate root in its
arguments and locals. Calling the collector's scan there would detect the proof's own root;
skipping that whole caller frame could hide an independent sender alias. `mid_turn == 0` says
only that the collector is not in its allocation path; it does not certify that generated frames
have unwound.

A pure classifier now tests a supplied span of machine words for raw, interior and tagged
candidate pointers using the same address interpretation as the collector. It changes no GC
marks; the C fixture exercises those forms and confirms the mark stays unchanged. This is a
component for a future safepoint scan, not a scan of a complete stack or register set. A clear
span has no accepting meaning, and the incoming probe still returns
`KILN_C99_GRAPH_ROOTS_UNENUMERATED`.

The actor transaction must establish a boundary *before* it loads the candidate pointer into
proof frames, capture every relevant register at that boundary, and scan from that frame through
the current OS thread's stack base. Failure to learn the stack base must refuse. The boundary
must include every older sender frame and exclude only the proof's own temporary values, while
the consuming handle and ticket have already been removed from sender roots with rollback on
failure. The current `setjmp`/`__builtin_unwind_init` collector path is platform dependent and
has not been validated as a complete, read-only register snapshot for ownership acceptance.
Until that protocol exists, no graph ticket may move.

### 2026-09-28 -- generated entry propagates failed root registration

Both synchronous and async generated entries now check the paired root registration before
key setup, module-state access or any body instruction. An active realm returns the registration
error immediately. A direct emitted-C fixture runs with an inactive realm, which has no
collecting realm heap and therefore skips registration. The focused C fixture forces a failed
registration, verifies that the result slot is untouched, then clears the failure and verifies
the body runs. The async emitter assertion covers the other entry branch.

This closes the silent registration-failure gap. It does not provide a positive ownership
certificate: the stack/register scan and actor transaction remain unresolved, and graph tickets
still raise `DataCloneError`.

### 2026-09-28 -- paired read-only visitors for generated roots

The provider now accepts a paired read-only visitor with a mark/teardown root registration.
The collector still invokes the mark callback; the incoming probe invokes only the visitor.
Generated unit callbacks enumerate their five frame registers, scalar/module wrappers enumerate
borrowed result slots, the graph launcher enumerates import/export/namespace arrays and active
module promises, and the shared graph wrapper enumerates its arrays and result slot. The
provider fixture checks that a paired callback reports an alias without invoking the mark hook;
the C fixture checks a candidate alias through a registered callback. The provider ABI advances
to 56 with 152 exports.

Legacy mark-only registrations still return an incomplete result, including any external
embedding that has not supplied a visitor. The stack and callee-saved registers remain
unenumerated, as do the consuming handle transaction and the proof boundary around its own
frames. At this point generated module registration still ignored a failure; the later entry
change above closes that gap. The incoming probe still returns no
accepting result and graph tickets continue to raise `DataCloneError`.

### 2026-09-28 -- provider-owned roots have a read-only visitor

The provider now visits its value slots, callable stores, continuations, HTTP event handles,
queued work and timers through a read-only callback. The incoming probe reports a separate
provider-root alias when one of those points into candidate chunks. The generated-C fixture
places a candidate cell in a provider value slot and callable store; the provider fixture also
checks queued work. Neither visitor calls the collector's mark hooks. If a generated module or
launcher has registered a mark-only root callback, the provider visitor reports incomplete
coverage without invoking it. The provider ABI advances to 55 with 151 exports.

This remains diagnostic: generated callbacks, the sender stack and the consuming actor
transaction are not proved, so the incoming probe still has no accepting result and graph
tickets continue to raise `DataCloneError`.

### 2026-09-28 -- read-only outside-edge scan across sender storage

The incoming probe now walks allocated blocks in the sender's large-block list, every used slab
slot, the active nursery chunk, filled chunks and promoted chunks. Candidate chunks are skipped.
For inline/sliced strings, bytes, cells, arrays and dictionary objects it checks their GC edges
and side pointers for aliases into candidate chunks, including interior addresses. A block with
an unaudited kind or a shaped object returns `KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED` rather
than an apparent clear result. The C fixture places an outside object's property into the
candidate and catches it in active, filled and promoted storage; it also checks slab and large
block aliases. No GC marks change during the scan.

This remains a diagnostic proof slice. Provider roots, generated callbacks, weak/shape side
tables, host handles and the machine stack are not yet covered, so a clear outside scan still
returns `KILN_C99_GRAPH_ROOTS_UNENUMERATED`. Graph tickets remain refused.

### 2026-09-28 -- incoming-root audit and executable refusal

`kiln_c99_graph_incoming_probe` first requires the narrow outgoing closure proof, then checks
the prelude's direct C roots and key-table side pointers: realm symbols and built-in prototypes,
module value blocks, region-closure stores, pending promise jobs, canonical keys (including
unpinned entries), hot keys, and the Object-prototype store. An interior pointer into a candidate
counts as an alias. The C fixture demonstrates aliases in a realm slot, pending job and unpinned
key entry, then demonstrates that even a closed candidate with none of these aliases returns
`KILN_C99_GRAPH_ROOTS_UNENUMERATED`. No positive transfer-ready result exists.

The remaining authorities, found by following the collector's root path, are:

| Authority | Missing proof seam |
| --- | --- |
| Rust provider `mark_realm_roots` | Provider-owned slots, stores, continuations, HTTP handles and queued work/timers have a read-only visitor. Paired generated callbacks are visited; any legacy mark-only callback forces an incomplete result. |
| Generated module and launcher callbacks | Their frame registers, graph wrapper imports/exports/namespaces and borrowed result slots have paired visitors. Active generated entries propagate registration failure. The consuming handle's `WeakMap` and ticket roots, and rollback remain unproved. |
| Every allocated block outside the candidate | The read-only scanner covers a narrow set of kinds and refuses shaped or unknown blocks. It still needs any permitted shape keys, weak collections and finalizer state before supporting those kinds. |
| Machine stack and callee-saved registers | Scan at the turn safepoint even when `mid_turn` is false. The proof's own frames hold candidate pointers and need a precise exclusion boundary; excluding an older caller frame could miss a real alias. |
| Actor transaction | Reserve mailbox and receiver capacity before detaching the handle, freeze both realms through proof and relink, and publish the receiver root before either resumes. |

This audit is intentionally fail closed. Passing the C-root scan does not authorize the chunk
mover or graph tickets; `prepareActorMessage` still raises `DataCloneError`.

### 2026-09-28 -- isolated handles no longer expose their graph

The actor module keeps an isolated factory result only in its private `WeakMap`. A handle's
dynamic-import marker is now a Boolean, and a graph transfer ticket retains the handle rather
than publishing the graph in a readable symbol property. The native actor probe checks every own
property on both objects and passes in the interpreter and native engines. This removes an API
leak; it does not provide the incoming-root proof or permit a graph move. The `WeakMap` entry and
the ticket's handle reference are still sender roots that the eventual consuming transaction
must remove at its safepoint. The ticket record is frozen; its consumed bit now lives in a
closure and changes only in one direction. The native actor probe also checks that code cannot
replace a buffer on a ticket or reset the bit after acceptance. The probe matched at 1 and 2
migrated realms with `KILN_NATIVE_GC_STRESS=1`.

### 2026-09-28 -- exact outgoing closure for a narrow candidate subset

An internal, read-only walker now checks that a sealed candidate's root reaches **every** block
in its dedicated chunks and that every edge stays in those chunks. It checks that the batch
contains the candidate's complete group, validates inline string backing and sliced-string
ranges, follows mutable-cell and slice-parent edges, and rejects any unknown block kind or
external pointer. A focused C fixture covers a closed cycle-free graph across two chunks and
refuses missing chunks, unreachable blocks, static-string edges, malformed string backing,
out-of-range slices, mismatched value tags, interior pointers, and unsupported kinds. The
supported kinds are inline/sliced strings,
raw bytes and cells. Arrays and ordinary objects still fail because their finalizer bits and
side allocations need an explicit ownership contract before the chunk mover can accept them.

This is the outgoing half only. It does not enumerate sender roots or edges from outside the
candidate, and it does not reserve the destination mailbox or atomically publish a new root.
`prepareActorMessage` still rejects every graph ticket with `DataCloneError`.

### 2026-09-28 -- dedicated candidate provenance at the chunk mover

The runtime now has an internal graph-candidate arena. Its allocations use separate off-heap
chunks, tagged with one unique candidate ID; ordinary nursery and parallel worker chunks have no
ID. Adoption records the sender collection epoch. The mover refuses untagged chunks, batches
that mix candidate IDs, and candidates whose sender has collected since adoption. A direct C
fixture covers those refusals as well as the successful two-chunk move. This establishes a
dedicated allocation provenance seam for step 1, but no source-level `isolate` factory uses it
yet. It still needs a complete block-edge and incoming-root proof at a turn safepoint before
graph tickets can be accepted. They continue to raise `DataCloneError`.

### 2026-09-28 -- sealed-chunk validation at the relink boundary

`kiln_c99_heap_move_chunks` now accepts only filled, never-collected nursery chunks. It verifies
their expected header and bitmap layout, walks every block from the fill start to the seal limit,
and reconciles that walk against the complete allocation bitmap and byte/count totals. A chunk
with any finalizer bit is rejected: finalizers can own external bytes or realm side-table entries.
The whole batch is checked before either heap changes, and the C fixture covers a corrupt count,
an extra bitmap bit, and a finalizer bit in a two-chunk move. This makes the storage primitive
stricter; it does not prove graph closure, missing incoming roots, or the actor transaction. Graph
tickets still raise `DataCloneError` pending the four remaining runtime steps below.

The TypeScript `isolate` handle retains the value in its module's `WeakMap`, while
`transfer(handle)` makes a ticket that retains the handle. Both are sender roots. A future proof
cannot simply exempt the ticket; the consuming transaction must remove every handle root at its
safepoint and move its private registry into engine-owned realm state. It must restore usability
on pre-publication failure. No code should reinterpret the current Boolean marker as an
ownership certificate.

### 2026-09-28 -- sender chunk unlink foundation; graph tickets still refuse

`kiln_c99_heap_move_chunks` now supplies the inverse of worker-arena adoption for sealed nursery
chunks. It validates the whole input batch (source membership, no duplicate or current bump chunk,
matching nursery geometry, counts and destination capacity), grows the receiver's region set before
any mutation, then removes each chunk from the sender lists and region set and enters it in the
receiver's. A C fixture verifies rejection leaves both heaps unchanged and a successful move keeps
the payload address and bytes while changing GC lookup ownership. This operation is internal and
has no JavaScript caller. It does not claim to prove that the chunks contain a closed graph.

The remaining runtime work is substantial and must precede enabling `transfer(isolate(...))`:

1. Allocate the factory's candidate graph in dedicated chunks, or evacuate it into such chunks
   and account for that copy. Ordinary nursery chunks are mixed. The proof must reject every
   allocated block in a candidate chunk that is outside the candidate graph.
2. Run the ownership check at a sender turn safepoint after the generated frame unwinds. Enumerate
   exact edges for each supported block kind, including internal slots and closure captures; reject
   unknown kinds, side tables, finalizers, local prototypes and edges into realm-local storage.
3. Enumerate **all** incoming roots and outside-graph edges after excluding only the consuming
   handle: C realm/module slots and queued jobs, provider-registered roots, host handles, timers,
   weak collections and finalizer state, and conservative machine-stack candidates. The existing
   GC mark API discovers reachability but does not expose a complete root/edge proof to the actor
   serializer. Neither a TypeScript ticket marker nor the old HIR spelling check supplies one.
4. Reserve receiver mailbox and heap capacity before detaching the handle. Keep both realms
   stopped from proof through sender unlink, receiver adoption and destination-root publication.
   Extend the transaction for any external bytes or side indices permitted in a future movable
   kind. On failure, leave the ticket and graph usable in the sender.

Until those seams exist, `prepareActorMessage` raises `DataCloneError` on every graph ticket.
That refusal is required for soundness and is not a completed step 3.

### 2026-09-27 -- object-graph proof boundary and HIR diagnostic correction

The old `kiln-hir` escape pass recognized **any** identifier or static member named `isolate`.
It rejected ordinary user functions and methods, while accepting object literals, cycles,
computed aliases, exported references, closures in nested expressions and references held by
host objects. It was neither scoped to `@kiln/concurrency` nor a proof that the sender could
release a graph. The pass and its frontend call were removed. The package's type declaration now
says what its implementation does: `isolate` and `transfer` are identity helpers today. They must
not authorize a native ownership move. This correction is not step 3's implementation.

A sound move needs a runtime proof at a turn safepoint, against the **actual** graph and all
sender roots. Let `G` be the transitive closure of the object being transferred through every
mutable object edge, including properties, elements, prototypes, internal slots, and closure
environments. Cycles within `G` are fine. Edges from `G` may lead only to other members of `G`
or to the shared immutable region. There must be no edge from a live sender object outside `G`
into `G`, and no sender root may name a member of `G` after the consuming handle is removed.
That includes aliases in stack frames, module bindings, queued jobs, host/provider handles,
weak collections, finalizer state, and objects reachable through a closure. The collector's
conservative stack scan can reject a safe transfer, but cannot be used to overlook a root.
Unsupported edge kinds or roots must make the proof fail. No mutator or collection may run
between proof, removal of the sender handle, and destination adoption. Destination capacity is
reserved before removing sender ownership, so failure leaves the sender intact.

The current nursery does not give an arbitrary graph its own chunks: unrelated blocks share each
chunk. Relinking one of those chunks would hand unrelated objects to the recipient and leave
sender references pointing into another heap. Step 3 therefore needs either dedicated graph
chunks from allocation onward, or evacuation into dedicated chunks (which copies), followed by
the inverse of `kiln_c99_heap_adopt_chunks` to unlink from the sender. The zero-copy path must
reject a graph outside dedicated chunks. Issue 12 must first define the consuming operation and
its source-level semantics; this issue then implements the proof and handoff against that API.

No graph-move path is enabled by this change. Existing `ArrayBuffer` transfer and worker-arena
adoption remain the working parts of primitive C.

### 2026-09-27 -- step 2 built (parallel issue 08, branch `work/parallel-tier3`)

Step 2 exists as a general primitive in `runtime/heap.c` ("Ownership transfer: chunks built off
the heap, adopted in one step"):

- `kiln_c99_arena_chunk(capacity, bitmap_words)` -- a chunk of the nursery's own kind (aligned to
  its size, header and three bitmaps laid out by the same `kiln_c99_chunk_lay_out` the nursery
  uses) that belongs to no heap. Any thread may make one.
- `kiln_c99_heap_adopt_chunks(heap, list)` -- on the heap's realm thread with no collection
  running: every sealed chunk (`limit` its fill, `alloc_count`/`alloc_bytes` its blocks, each
  block with its header, allocation bit and, for a kind finalised from birth, its finaliser bit)
  becomes the heap's, or none does. It reserves room in the nursery's region set first (so no
  insert can fail half way), enters each base, widens the nursery's and the heap's address bounds,
  queues the chunk on the filled list the next collection settles (reset when nothing in it
  lives, promoted whole when something does), and adds the counts. Nothing is read or written per
  block.
- `kiln_c99_arena_chunks_free(list)` -- a list nothing references, returned.

Soundness rests on who can see a chunk before adoption: only its builder -- it is in no heap's
set, so the heap's collector neither finds nor frees it -- and on the builder's protocol ruling out
a collection while objects in the chunk point into the heap unmarked. For the parallel section
that protocol is the heap's `parallel_paused` state (a collection due while it is set aborts).

What steps 1 and 3 still need from it: a transfer between two realms' heaps also needs the
*sender* to give chunks up (unlink from its region set and lists, with a proof it keeps no
reference), which is `kiln_c99_nursery_free_region` minus the free; and chunks of the sender's
own nursery hold unrelated blocks, so an object graph would move whole chunks only after being
evacuated into chunks of its own -- a copy, which primitive C exists to avoid. The arena case
avoids both: its chunks were built for the transfer and hold nothing else.

### 2026-09-29 -- candidate manifests and receiver mark authority

Candidate chunks now belong to a dedicated intrusive allocation-group manifest. Discovery and
factory completion visit that group rather than unrelated filled nursery chunks; outgoing graph
membership uses a region pointer set. Collector reclamation, failed factories, private-arena
disposal, detachment and adoption remove membership explicitly. The manifest does not root dead
objects. A builder pin keeps its metadata valid while an empty group can still receive chunks.

The metadata costs 32 bytes per candidate group on this 64-bit build, one pointer per heap/private
arena, and increases the aligned nursery header from 128 to 192 bytes per default 256 KiB chunk.
Ordinary object headers are unchanged.

A strengthened handoff fixture exposed a collector bug: sender and receiver could use the same
mark epoch, causing the receiver to mistake a sender mark for a completed local traversal. Nursery
mark authority now requires both the block epoch and the receiving chunk's local mark bitmap.
Adoption resets chunk mark authority without walking payloads. The fixture forces collection in
both realms with colliding epochs, and verifies live multi-chunk objects/strings survive while
orphan allocations are reclaimed. A second fixture checks removal of an interior manifest member
and held-byte accounting during handoff.

Focused checks: 259 backend unit tests passed (one ignored), 36 emitted-C tests passed, and the
strengthened capsule fixture passed separately. Full native collector stress: 317 matched, one
existing ShadowRealm refusal, zero diverged, 42 explicit stress skips. Three affected actor probes
ran three times each at 16 and 64 realms: three matched, zero diverged.

This is partial step-3 progress. The incoming-alias proof still scans sender roots and the heap;
a complete candidate-lifetime escape ledger and standalone large-block ownership remain open.

Final frozen gate on `b62170d1`: all ten steps passed in 367 seconds, with 90 crate suites,
2,290 passing tests, zero failures and nine ignored. Native parity was 359 matched, one existing
refusal and zero diverged. Routine Test262 passed its floor with 97,116 passing cases and no
regressions across 122 chunks (102,522 selected, 116 deferred). Gate snapshot:
`112ad22e98ed32d2dbb8f444af730ce7d806e8d02e1112dfbf3ca337aa8a5993`; CLI:
`015d183ec376745c41d9ed52e12ea420544c83566caa80c6079ca35fc91881e8`.
Evidence: `target/gate-logs/run.07slz2KU/aggregate.json`.

After all goal build/test lanes finished, three alternating rounds of the unchanged warmed
JSON workload compared the saved `cdecff41` binary with `b62170d1`. Both returned `60050000`;
before medians were 65/63/64 ms, after medians were 62/62/63 ms. This selected allocation-heavy
workload shows no added cost from the mark-authority fix; it is not a general collector
performance claim. Raw cells and binary digests are retained in
`performance-evidence/graph-group-gc-2026-09-29.json`.

### 2026-09-29 -- one indexed certificate per direct handoff

The outgoing walk now retains its candidate-region index for incoming membership checks.
Pointers outside those regions are rejected before a GC interior lookup; candidate interior
pointers retain the collector's exact recognition rules. The index is bound to the original
manifest vector, so a diagnostic checking a different batch uses its own membership rules.
Owned side allocations keep their sorted-span check. The proof owns and disposes both indexes.

Once all C, machine, outside-heap and provider root checks pass under the same paused sender
lease, a non-allocating consumer constructs the capsule directly from that validated manifest.
It sums chunk counters and reuses the outgoing walk's dictionary-key flag instead of repeating
the outgoing graph walk and scanning every payload again. A consumer refusal restores escrow
and leaves vector disposal to the proof; acceptance transfers vector ownership to the capsule.
Private-arena preparation and direct certification share one exact root-representation check.
No mutator or collection runs between validation and consumption.

Focused checks passed: five candidate-storage cases, the escrow certificate fixture (including
consumer refusal followed by successful re-admission), and all 36 emitted-C cases. The ABI
snapshot was re-recorded using INSTA_UPDATE=always.

Three alternating quiet rounds of the public ~1 MB actor graph benchmark used five samples
per case. All 48 cells returned checksum 21202207. Direct handoff fell from 14.365 to 11.208 ms
(about 22%); direct construction-plus-handoff fell from 19.859 to 16.666 ms. Ordinary copy was
12.176 vs 12.046 ms, prepared total 8.864 vs 9.034 ms, and factory construction 5.348 vs 5.438 ms.
Construction conditions differ across paths: the copy/prepared rows reuse a prebuilt source,
whereas direct factories construct a fresh graph each time. This establishes a handoff-phase
improvement, not a general end-to-end win over cloning. Digests and all cells are retained in
`../performance-evidence/graph-proof-manifest-2026-09-29.json`.

Step 3 remains open: the incoming proof still walks sender heap/roots, and standalone large
direct allocations still need ownership metadata and transfer support.

Final gate on `3ad565da` passed all ten steps in 347 seconds: 90 crate suites, 2,290
passed, zero failures, nine ignored; native 359 matched, one existing refusal, zero
diverged; routine Test262 97,116 passing with no regressions across 122 chunks
(102,522 selected, 116 deferred). Snapshot:
`8783b2b233222201fb8e00315b1b36b8ce1cf053bf76a2dcdfa0d31632c05b44`; CLI:
`212995779f9dbc9919f4869cd346f342e9cd55cfeb14175cf473c045f425ca80`.
Evidence: `target/gate-logs/run.inFns20g/aggregate.json`. Full collector stress then
passed: 317 matched, one existing ShadowRealm refusal, zero diverged, 42 explicit
stress skips. The three direct/cancellation/concurrent-stop actor probes matched
at 16 and 64 realms, three repeats each, with zero divergence.


### 2026-09-29 -- standalone direct allocations share the candidate manifest

Source factories now register standalone allocations in their existing allocation group through
cold heap-local owner metadata. Ordinary GC block headers keep their size. A new kind flag and
matching typed payload mask identify the cold lookup path. Owner links are not collector roots:
sweep, explicit release, generic heap moves and teardown remove them. An empty builder pins its
group, and a closed group is released only after both its chunks and standalone blocks are gone.
A failed or collected factory drops provenance without discarding values exposed to its caller.

The outgoing walk builds one exact large-block index and allocation vector, verifies ownership
and current epoch, then applies the same audited block-layout walk used for nursery members.
Owned allocation spans cover headers and interior aliases during the incoming proof. Selected
large members are excluded from the outside-heap walk; outside references still forbid transfer.
The capsule consumes both manifest vectors only after the full incoming proof succeeds. Receiver
preflight reserves large-table capacity; sender detach unlinks exact blocks without copying
payloads; adoption resets marks before publishing the receiver root. All fallible operations stay
before unlink, preserving the existing paired-realm transaction and cancellation protocol.

Focused fixtures cover weak metadata reachability, failed-factory preservation, real standalone
allocation, header/interior aliases, sender collection and receiver collection with colliding
mark epochs. The public actor probe transfers a 300,000-character string in an ordinary object,
refuses a retained object alias, and retries the same ticket after release in both engines.
Cancellation probes now use large graph strings at snapshot-abort, publication-abort and
stopped-receiver boundaries. Standalone scalar-string roots remain outside the interpreter's
public direct-transfer contract; the native storage fixture checks the zero-chunk manifest
internally without claiming new cross-engine API support.

The incoming proof still enumerates sender heap and roots. A candidate-lifetime escape ledger,
including post-factory stores, remains the next required step for candidate-scaled certification.


Verification on `1f3b2e3d`: the frozen gate passed all ten steps in 574 seconds.
90 crate suites reported 2,292 passed, zero failed and nine ignored. Native parity:
360 matched, one existing refusal, zero diverged. Routine Test262: 97,116 passing,
no regressions across 122 chunks; 102,522 selected and 116 explicitly deferred.
Snapshot: `4915757e60913033308f547b4e4ede41f5940ed0359e9d1d85e1e4b6f8b69417`.
CLI: `96acd1a92ec3c12266b3a9968bd466155bc3c97e6962cdaccbe7db6440b080b9`.
Gate evidence: `target/gate-logs/run.C41CxahW/aggregate.json`. Collector stress:
317 matched, one existing ShadowRealm refusal, zero diverged, 43 explicit skips.
Large-string/direct/cancellation probes matched at 16 and 64 realms, three repeats
per count, zero divergence. The allocation fixture also checks a surviving large
string after its last nursery member is reclaimed. Durable counts and digests:
`../performance-evidence/graph-large-allocations-verification-2026-09-29.json`.


### 2026-09-29 -- candidate lifetime owner ledger and publication inventory

Implemented the next incoming-proof foundation on `work/graph-escape-ledger`. Candidate groups
retain weak owner-block witnesses for their entire lifetime, including stores after a factory
closes. Cell writes, runtime and emitted array/object slots, bulk copies, closure captures,
Map entries, Proxy fields, Promise reactions/settlers and bound captures use shared recording
authorities. Rechecking an owner's current fields permits retry after aliases are cleared and
survives side-buffer growth. Reclaimed witnesses resolve through live heap indexes before any
field dereference. Metadata failure refuses ledger proof; metadata does not root the graph.

Private parallel workers never update realm metadata. Adoption indexes the entire batch, then
records published owner layouts on the realm thread using the existing incoming field walker.
A temporary observer visits every field without short circuiting; unknown publication layouts
mark ledger proof incomplete. The incoming inventory now includes Proxy target and handler,
without invoking traps. Repeated stores deduplicate owner witnesses explicitly: the existing
collector pointer table assumes its callers already supply unique pointers.

Four executable fixtures cover post-factory cells, clears/retry, weak reclamation, object/array
bulk copies and growth, two-chunk parallel publication, unknown-layout refusal, and collection/
Proxy/async holders including hashed Map updates. Backend verification: 265 unit tests passed,
0 failed, 1 ignored; 36 emitted/snapshot tests passed after automatic snapshot recording and a
reviewed prelude ceiling adjustment (66,278 measured lines). Logs remain in the owned worktree
under `target/ledger-backend.log`, `target/ledger-holders-final.log`, and
`target/ledger-emitted.log`.

This is not complete candidate-scaled certification: production admission still uses the full
outside-heap scan. Remaining work includes generator/frame and host mutation coverage, a complete
store inventory with enforceable authorities, stale-witness compaction, oracle comparisons and
candidate/sender-heap scaling measurements, plus native, collector-stress and realm verification
before landing. No new transfer claim is made by the partial ledger.


### 2026-09-29 -- weak-witness reclamation and suspended-frame stores

Candidate groups now have a realm-owned intrusive ledger list. Disposal unlinks the group;
normal successful collection prunes witnesses only after live heap indexes reflect reclamation.
An empty table is released, and a sparse table is rehashed down to its current owner count.
Rehash allocation failure preserves the complete existing table. Publication failure state can
reset only after the last affected candidate has lost provenance or died.

Generator/async frame construction and mutations now record locals, operand spills, receiver,
sent values, delegate state, promises and request queues. Generated bodies give heap-backed
locals their frame owner, while automatic locals and module roots use a null heap owner (the
latter remain independently checked by the complete root inventory). Materialized literals and
new-cell publication use the same local-store authority. Both compact and wide emitted stores
share one runtime pointer-recorder bridge; numeric/immediate and null-owner writes stay inline.

The lifetime fixture now covers collection with a retained candidate, reclamation of all old
owners, exact nursery address reuse, 100 transient holders, shrinking a 256-slot witness table
to 64 slots, and releasing the final table. A new executable fixture runs the generated generator
body, clears its original capture, and proves both suspended locals and a spilled operand still
prevent transfer until cleared. An initial fixture crash was diagnosed with AddressSanitizer:
the top-level entry intentionally stringifies returned function values; the fixture now calls
the generated factory body directly rather than treating that boundary string as a closure.

Verification: 266 backend unit tests passed, 0 failed, 1 ignored; all 36 emitted/snapshot tests
passed. Snapshots were re-recorded through INSTA_UPDATE=always. Evidence remains in the owned
worktree's `target/ledger-lifetime-backend-final.log` and `target/ledger-suspended-final.log`.
Production incoming admission is still the full outside-heap scan. Constructor/host coverage,
enforced store inventory, oracle comparisons and scaling measurements remain open.


### 2026-09-29 -- lifetime ledger native module verification

Built the release CLI at `9fd58cb1` and ran four selected probes through `scripts/native.sh`:
actors-direct-graph, generator-protocol, async-generators and graph-emitter-in-and-cells.
Ordinary parity: 4 matched, 0 refused, 0 diverged. The same subset under collector stress:
3 matched, 0 refused, 0 diverged, 1 explicitly skipped direct-factory probe. This checks the
shared pointer-recorder bridge through separate native runtime/module compilation and the
suspended-frame paths with a real collector. It is subset evidence, not a full gate or full
stress claim. Durable counts, implementation tree, CLI digest and log digests are recorded in
`performance-evidence/graph-ledger-lifetime-verification-2026-09-29.json`.


### 2026-09-29 -- constructor publication and interior-pointer stores

Raw pointer fields now share a store authority alongside compact and wide JS values.
Candidate lookup recognizes nursery interiors and interior addresses in candidate-owned large
allocations. Function/prototype property-store constructors publish through an owner-aware
output-slot helper. Dictionary clones record copied descriptors; request/response shaped slots,
arguments objects, symbol descriptions, process completions and string-split output slots use
the same recording authorities. These changes retain the existing mutation semantics.

An executable fixture checks large-string interior aliases, nursery interior aliases, clearing
and retry, and publication of a candidate property object into an older closure's own-store
slot. Both the ledger and the full scan observe those aliases. Verification: 267 backend unit
tests passed, 0 failed, 1 ignored; all 36 emitted/snapshot tests passed. The ABI snapshot was
automatically re-recorded. Log: target/ledger-constructors-backend.log in the owned worktree.
Production admission remains the full scan pending complete host/store coverage and scaling
verification; this is not a claim of completed candidate-scaled certification.


### 2026-09-29 -- one host-reference inventory for collection and transfer

The nine host payload layouts (server, socket, client request, response, completion, fs and
process completion, next tick and timer) now use one typed field inventory for both collector
tracing and the outside-edge walker. Publication recording already uses that walker, so all
three consumers share the host field list. The list distinguishes wide language values, GC
pointers and malloc-owned value arrays; malformed nonempty null arrays refuse proof. Host
handles and malloc-owned transport/event buffers remain outside the language heap inventory.

Host callbacks, argument copies, cached response values, linked payloads and object constructor
output slots now record incoming candidate owners. A new executable fixture retains four
independent host aliases, compares ledger and full-scan rejection as each is cleared, and checks
retry after all four clear. It exercises the real next-tick argument-copy constructor with a
fixture queue that retains submitted work instead of the default immediate-disposal stub.

Verification: 268 backend unit tests passed, 0 failed, 1 ignored; all 36 emitted/snapshot tests
passed. Snapshot updates were recorded automatically and reviewed. Log:
target/ledger-host-backend.log in the owned worktree. Production admission remains the full
heap scan: the shared host inventory closes a coverage gap but does not prove all store sites.


### 2026-09-29 -- constructor/host native and realm verification

The release CLI built from 903f322e passed eight selected native probes: direct actor graphs,
generator protocol, async generators, emitted graph cells, timers, process next ticks, URL
surface and HTTP request lifetimes. Ordinary parity: 8 matched, 0 refused, 0 diverged. The same
subset under collector stress: 7 matched, 0 refused, 0 diverged, 1 explicit direct-factory skip.
Timers, next ticks and URL surface also matched at 8 and 16 concurrent realms, twice per count
(3 probes matched, 0 skipped, 0 diverged). This is focused evidence, not a full final gate.
Implementation/CLI and log digests plus counts are recorded in
performance-evidence/graph-ledger-host-verification-2026-09-29.json.


### 2026-09-29 -- production-boundary ledger oracle and typed store audit

KILN_GRAPH_LEDGER_AUDIT=1 compares ledger observations (including the separate local-shape
audit) with the full outside-heap scan at actual request certification boundaries. Any
disagreement emits a diagnostic and refuses certification, including unknown/failed ledger
metadata when the full scan answers clear. Production admission remains the full scan. The option is cached per
thread, introduces no per-realm field, and does not change successful program output.

A new executable fixture deliberately bypasses a cell store authority, checks that audit mode
refuses the mismatch, then records the same store and checks alias rejection and retry after
clear. The collection fixture now exercises compact insertion, unhashed and hashed updates,
candidate keys, deletion and the dictionary new-accessor branch against both proof paths.

A Clang typed-assignment inventory of the combined prelude found 2,001 reference/output writes
(including stack/output temporaries and non-GC metadata). It exposed missing compact Map entry
writes, dictionary accessor descriptor writes, JSON dictionary values, Atomics.waitAsync
completion promises, EventEmitter table publication, lazy array/collection/promise/bound property
constructors, sealed-slot copies and arrow captured-arguments output slots. These now use the
shared store/publication authorities. This diagnostic inventory is not yet an enforced complete
store inventory: pointer fields, bulk copies and output-slot calls still need classified coverage.

Deleting a compact or generic Map/Set entry now clears its key and value after hash removal.
The incoming walker deliberately inspects dead entries, so retaining their stale addresses
previously prevented a safe retry after deletion. Clearing preserves insertion order/dead-slot
semantics while releasing the inaccessible references. Shaped accessor deletion can still leave
an unreachable pair until collection; the new-descriptor fixture explicitly uses dictionary
storage to exercise the previously missed branch.

Backend unit verification passed 269 tests, 0 failed, 1 ignored. The emitted prelude measured
66,420 lines; the reviewed ceiling moves from 66,400 to 66,450 for the audit helper and store
coverage growth. ABI snapshots were automatically re-recorded. The emitted suite passed all 36 cases
after that ceiling update (target/ledger-audit-emitted-final.log). Logs remain under target/ledger-audit-* in the owned worktree.


### 2026-09-29 -- strict ledger oracle verification

The audit now requires exact agreement, including refusing incomplete ledger metadata when
the full scan succeeds; unknown observations cannot masquerade as completed verification.
The focused fixture checks this in addition to an unrecorded store, recording and clearing.

At 7e5eccc5, strict audit release verification passed 15 selected native probes (0 refused,
0 diverged), and the same subset under collector stress passed 14 (0 refused, 0 diverged,
1 explicit direct-factory skip). Direct graphs, large direct strings and collections matched
at 8 and 16 concurrent realms, twice per count. The native-only realistic graph succeeded;
the native-only direct probe under collector stress verified rollback and retained-alias refusal.
Durable counts, implementation/CLI digests, precise unit-suite scope and log digests are in
performance-evidence/graph-ledger-audit-verification-2026-09-29.json.

All changes remain on work/graph-escape-ledger. Complete enforced store coverage, scaling
measurements, production certification promotion and the final gates/main push remain open.


### 2026-09-29 -- pointer publication authorities and source guard

Array/object element, slot and property-storage pointers, closure upvalue pointers, generator
local/stack/queue pointers and typed-array/DataView buffer pointers now publish through the
shared raw-pointer authority. This includes private-arena construction; its existing no-write
guard skips realm bookkeeping, and adoption still records the complete published layout. Whole
property descriptors use one store authority that records keys plus data/accessor values.

A tokenizer-based source guard rejects direct and indexed assignments to the selected audited
reference fields and compact collection entries, including macro RHSs and compound writes,
while ignoring comments, strings and reads. It found the whole-descriptor insertion/deletion
shifts, now routed through the descriptor authority. This guard covers those direct syntax
forms; arbitrary aliases, bulk-copy calls and constructor output slots remain separate audit
requirements. No claim of complete enforcement or ledger admission promotion is made.

Verification: 271 backend unit tests passed, 0 failed, 1 ignored; all 36 emitted/snapshot tests
passed. The ABI snapshot was automatically recorded. Log:
target/ledger-pointer-authorities-backend.log in the owned worktree. Native/collector parity
and isolated before/after timing are checked next.


### 2026-09-29 -- ordinary pointer-store fast path

Isolated native binaries from 7e5eccc5 and cd24d34b were built before timing, then alternated
for five trials per workload. Every checksum matched Node; timings use the in-process warm
harness. Initial medians: class-poly 1.750 -> 1.719 ms, object-spread 0.1914 -> 0.1914 ms,
object-literal 3.750 -> 4.125 ms. The literal regression is material (~10%), so it is not accepted.

Raw pointer publication now checks the active candidate count inline before calling the cold
ledger recorder. No-candidate constructors retain the store without entering that function.
Candidate, weak-witness and no-write semantics remain unchanged. The candidate fixtures and
object-literal timing are checked after this adjustment; initial trial records are retained in
target/pointer-authorities-bench.json. These comparisons are against the prior ledger version,
not the historical pre-step-3 performance target.


### 2026-09-29 -- validated cache-only pointer-store path

The first active-count fast path did not remove the ordinary literal regression: five
alternating trials measured 3.750 -> 4.250 ms. The pointer store now uses the existing
generation-validated `gc_heap_if_cached` authority, keeping the common no-candidate path
free of heap-getter calls. Missing/stale caches resolve the current realm through the
recorder; active candidate groups still use the full recorder. The no-write guard remains
before any cache access. No incoming-store coverage was removed.

Five alternating warm, in-process trials against the saved 7e5eccc5 binary now measure
3.8125 -> 3.8125 ms, with matching 600130000 checksums. Both builds use native-release
without the Node adapter and reuse identical generated-C/app objects (15,027,912 /
8,125,584 bytes). An initial Node-adapter build changed those inputs and is excluded from
this comparison. Raw trials: target/pointer-cached-plain-bench.json. This removes the
measured pointer-authority regression; it does not establish all transfer scaling targets.
Focused candidate, reference-store and native/stressed checks are run on this version.

Verification on the cache-only version: all eight candidate fixtures and both source guards
passed. The selected native corpus reported 15 matched / 0 refused / 0 diverged; collector
stress reported 14 matched / 0 refused / 0 diverged with one declared candidate-provenance
skip. Three direct-graph/large-string/collection probes matched at 8 and 16 realms, twice
each. Exact code identity, CLI/binary digests, trial records and log digests are retained in
[the pointer fast-path evidence](../performance-evidence/graph-ledger-pointer-fastpath-verification-2026-09-29.json).
The full final gate and production ledger promotion remain due.


### 2026-09-29 -- nested buffers and destination wrapper publication

The source guard now rejects direct stores through promise reaction fields, object descriptor
key/value/getter/setter fields, captured inline upvalues, suspended locals and async queues,
exception-buffer pointers, and regexp cache storage. It found nine buffer/aggregate writes
and three descriptor-key writes. Those use pointer/value authorities; reaction publication
copies its numeric continuation flag separately from its three recorded value fields.

Moving an existing wrapper or promoting a nursery/slab wrapper now records its initialized
layout in the destination heap after indexing. A focused two-heap fixture retains a live
candidate in the destination and publishes a copied cell plus a relinked large cell while
the source heap remains active. Both incoming aliases are witnessed; clearing both permits
retry with strict full-scan agreement. Destination publication uses the explicit heap, not
the source thread-local current realm. Unknown layouts still invalidate proof.

Verification: 272 backend unit cases passed, one ignored; all 36 emitted/snapshot cases
passed. The selected native corpus reported 15 matched / 0 refused / 0 diverged; collector
stress reported 14 matched / 0 refused / 0 diverged, with the same one explicit provenance
skip. Three direct-graph/large-string/collection probes matched at 8 and 16 realms twice
each. Logs are target/ledger-destination-{backend,native,stress,realms}.log in the owned
worktree. The guard remains partial for pointer aliases and bulk/output seams; production
admission still uses the full scan.


### 2026-09-29 -- header setters and bulk ownership authorities

The reference guard now scans both runtime headers as well as C domain files. It found the
decoupled private-field cache setter's direct slot write; that hit now uses the shared value
store authority. The public direct-graph probe warms the setter with ordinary objects, writes
a candidate after factory completion, verifies alias refusal, clears the private slot, and
retries successfully. A separately captured strict-audit run contains no ledger/full-scan
disagreement diagnostic.

A new bulk-copy guard rejects raw memcpy/memmove involving audited reference buffers and
explicit value/descriptor/reaction types. Sixteen paths now use value or descriptor authorities,
including dictionary clone/growth, array growth and shifts, EventEmitter removals and promise
reaction growth. Descriptor output readers produce a temporary descriptor before publication
through the destination object's store authority.

Ownership distinguishes operations rather than forcing every byte move through bookkeeping:
cross-owner copies record incoming witnesses; same-owner resize and shifts keep the existing
owner witness. A guard rejects relocation calls whose source is another owner's buffer or an
unproven pointer alias. Inline/tail layout pointers use an offset-based interior authority:
by construction they cannot name another allocation or candidate group. Borrowed pointers
still use the recorder. This restored the constructor's ordinary path without weakening
external-edge observation. Full admission remains the outside-heap scan.

Isolated native-release literal timing across five alternating trials is 3.6875 -> 3.625 ms
with checksum 600130000. Earlier intermediate bulk versions showed approximately 3--7%
regressions; those are superseded by the ownership-aware version, not accepted as final.
The initial bulk object-spread comparison was 0.1914 -> 0.1895 ms, with matching checksums;
it precedes the final self-owned constructor change and is not claimed as final-source timing.
The final full backend run passed 276 unit cases (one ignored) and all 36 emitted/snapshot
cases. ABI snapshots were re-recorded automatically. Native subset: 15 matched / 0 refused /
0 diverged. Stress subset: 14 matched / 0 refused / 0 diverged, one explicit provenance skip.
Three graph/large-string/collection probes matched at 8 and 16 realms, repeated twice.
Logs: target/ledger-bulk-final-{backend,native,stress,realms,private-audit}.log.
Arbitrary aliases, generic bulk copies, output slots and complete layout publication still
need the remaining inventory before production ledger promotion.


### 2026-09-29 -- wide buffer publication and weak native root witnesses

Bound-function target, receiver and argument-buffer fields now use store authorities directly.
Wide argument buffers share one memcpy-based publication helper, also used by queued next-tick
arguments. Recording occurs only for pointer values while a candidate group exists; private
worker writes retain the no-write guard. The bound constructor no longer depends on a later
whole-layout walk to repair raw writes. Unique description/diagnostic/reaction fields and
known bound fields are added to the source guard.

The C root audit now checks weak native resource registrations' handle and native payload
addresses. They are weak for GC, but their stored addresses cannot remain in a sender when
their allocation moves. The executable fixture covers each address independently, then
removes registration and verifies clear retry; disposal counts are checked. A producer guard
records the sole current native payload seam: a private prepared arena whose graph is closed
and owned independently of the sender heap. New direct registration sites require a hidden
reference/lifetime inventory. This does not claim arbitrary native payload tracing.

Verification: the wide-buffer version passed 276 backend unit cases (one ignored); after the
weak-root addition the dedicated fixture passed, and all eight source/producer guards passed.
All 36 emitted/snapshot cases pass on the final code. The initial wide helper raised the
measured prelude to 66,511 lines; its single ABI entry was recorded automatically, and the
ceiling now allows 66,540. Final selected native/stress subsets report 15/14 matched, no
refusals or divergences and the existing one stress skip. Two actual bound-call probes
matched normally and under stress on the wide-buffer version. Two prepared-transfer probes
matched normally and under stress on the final weak-root version. Logs: target/ledger-wide-*
and target/ledger-resource-{native,stress}.log in the owned worktree. Final whole-program
gates and complete incoming-store coverage remain due; admission still uses the full scan.


### 2026-09-29: candidate-scaled admission implementation checkpoint

The `work/reference-metadata` batch implements ledger-based heap admission, removes canonical
key/shape sender-wide scans from the optimized path, and retains the full walk under strict
audit and the explicit `KILN_GRAPH_LEDGER=0` baseline switch. The authority changes and honest
coverage limits are in `../transfer-scaling-authorities.md`. Focused source guards and candidate
fixtures pass; public scaling curves, production probe parity, stress and the final frozen gate
remain acceptance work. This checkpoint does not resolve issue 10 or bootstrap issue 08.
