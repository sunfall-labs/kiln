# 08. The shared immutable region

Status: deferred (remaining broad bootstrap sharing; existing region improvements are landed).
Current measurements: the region holds all 327 once-made bootstrap closures; the current
scheduled core-realm slope is about 97 KiB per added realm, from 688 KiB before the region;
promoted nursery page reclamation lowers the public held-actor footprint by about 215 MiB at
1,000 actors on Linux, and direct process.env construction brings the held slope to about
530 KiB per actor; the broader Node/actor bootstrap graph remains --
see Comments.
Blocked by: 04

ADR 0070's primitive B. A region of objects that never change, readable from every realm and every
scheduler thread without copying, through a read path that writes nothing. That read path is the
same one ADR 0069 needs (`docs/issues/parallel-builtins/` issue 06): build it once, here.

1. **Built-ins and bootstrap first.** Build the intrinsics and the bootstrap heap once per process
   into the region, and have each realm start from it instead of rebuilding it. Where a realm
   changes a built-in (which the language allows), the change is copy-on-write into the realm's
   own heap, so every realm still sees an ordinary, mutable set of intrinsics (ADR 0059).
2. **Then user data**, for 11: freezing a value graph into the region.

A human reviews the design first: it changes how intrinsics are materialised and what the
collector may assume about pointers into the region.

## Acceptance

- The memory of a second realm, measured as in 01, drops by the bootstrap heap's size.
- Modifying a built-in in one realm is invisible to every other, in a probe.
- `mise run native-stress` clean.

## Comments

### Shipping decision: defer broad bootstrap sharing

The user chose to ship the usable actor and parallel model after completing transfer validation.
The remaining process-owned bootstrap blueprint, realm identity overlays and direct-plan
instantiation are deferred. Their prototype model compiled but was never connected to the
runtime, enabled or measured as a memory/startup improvement. The draft is preserved in the
owned worktree stash named `Deferred bootstrap sharing draft; user chose transfer validation
and shipping`; it is excluded from the transfer landing.

Existing region, shared layouts and actor startup improvements remain in production. This
deferral does not claim the tens-of-KiB or microsecond-startup targets have been met. Reopen
the broad representation work when application measurements justify its implementation cost.


### 2026-09-29 -- socket-capable program gate and collector stress passed

The full gate and native collector stress passed on `e4bda4df`; exact counts,
coverage and evidence paths are in `../program-verification.md`. The affected env
layout and actor graph/shutdown probes also matched at 1, 2 and 16 realms, three
runs each. This clears the earlier sandbox verification blocker, while the broad
bootstrap sharing and density target remain open.

### 2026-09-29 -- object inventory bounds a property-only density change

The existing opt-in heap profiler can now print each retained ordinary object's key spellings,
layout and side capacities at the second actor turn boundary with
`KILN_HEAP_PROFILE=1 KILN_HEAP_PROFILE_OBJECTS=1`. It does not print property values or change
the ordinary object layout. On the fourth actor in the native-release public four-actor workload,
it found 198 objects with 42,936 bytes of GC payload. The same pass reported 23,136 bytes of
dictionary-property capacity, 5,120 bytes of shaped slots, 3,744 bytes of sealed values and
1,024 bytes of object elements: 75,960 bytes including the object payload. Two of the larger
dictionary layouts expose 118 built-in names with capacity 234 and 82 typed-array methods with
capacity 128. The program returned the expected checksum of 10.

This key-only inventory does not establish allocation provenance or graph ownership. It bounds
the saving available from ordinary-object headers and these side stores; a property-table-only
change cannot close the roughly 530 KiB public-actor RSS slope. The first broad bootstrap
candidate still needs the object, closure, captured-cell, array and allocator-page ownership
audit described in `../bootstrap-image-design.md`. Raw detail is in
`target/actor-object-profile.err`. Backend tests passed 257 unit cases (one ignored) and 36
emitted-C cases; the ABI snapshot was regenerated with `INSTA_UPDATE=always`.

### 2026-09-29 -- full native stress is gated by this sandbox's socket restriction

`mise run native-stress` was attempted on `cb6bc3e6` with eight jobs and the full native
corpus. It stopped at `app-bun-serve-global`: the interpreter could not start the server and
reported unsettled top-level await, so the runner correctly refused to compare it with native.
This sandbox rejects socket creation with `EPERM`; the failure is not a collector verdict.
The full task must run in a socket-capable environment before issue 08 closes. The focused
process-environment layout probe has already matched at 1, 2 and 8 simultaneous realms under
every-allocation GC stress, as recorded below.

### 2026-09-29 -- share the initial process.env layout without slowing actor startup

The native process now registers the host environment's initial key spellings before the first
realm, after the fixed built-in keys. Its first realm publishes a complete `process.env` key and
attribute descriptor; later realms construct their own values directly against it. Reordered
known keys keep realm-local ordinals; an added host key takes the ordinary dictionary path. The
descriptor owns no env value or realm object. A process-owned open-addressed key index and a dense
insertion-order projection keep worker startup's env clone and overwrite linear in the number of
keys; the first
draft's descriptor scans made 1,000-actor spawn take about 3.8 seconds and were removed.

Three alternating native-release public actor runs at 100 and 1,000 held actors compared the
pre-change actor binary from `2a800ced` with the indexed implementation. Median held RSS changed
from 79,708/615,352 KiB to 77,220/554,688 KiB, reducing the added-actor slope from **595.16 to
530.52 KiB**. The 1,000-actor spawn median changed from **544.3 to 471.6 ms**. All runs checked
the expected 5,050 and 500,500 reply sums. The earlier baseline commit differs from the
candidate by opt-in heap profiling and direct graph-proof work that the capacity program does not
exercise. A final-source spot-check again measured about 595 versus 530 KiB per added actor,
with 1,000-actor spawn at 535 versus 480 ms. Raw records are in
`target/actor-env-index-measure.jsonl` and `target/actor-env-final-measure.json`.

The backend suite passed 257 unit and 36 emitted-C cases. Both new env probes matched the
interpreter and native backend. The layout probe matched at 1, 2 and 8 simultaneous realms under
every-allocation GC stress; the worker isolation probe matched at those realm counts normally.
This is a material actor-enabled bootstrap slice, but the remaining roughly 530 KiB slope still
requires sharing the broader object and captured-state graph. The full program gate remains due.

An opt-in second-turn heap sample of a later actor attributes the saved live storage more
precisely. Retained strings fell from 582/22,606 B to 353/14,101 B, and ordinary property-array
capacity from 35,424 B to 23,136 B; sealed value capacity grew from 1,012 B to 3,732 B.
The sample used two GC slabs instead of three. Its 198 ordinary objects, 181 closures, 159
arrays and 195 cells were unchanged, as were its three promoted nursery chunks. The RSS change
therefore includes allocator residency beyond the 18 KiB net decrease in counted payload and
side capacity. The before/after samples are in `target/actor-heap-{before,after}.err`.

A follow-up tried sharing short ASCII initial environment *values* through the process string
table and retaining those values when worker startup cloned equal strings into the child. Three
interleaved 100/1,000-actor rounds measured a slope of 529.99 KiB before versus 527.03 KiB
after, while the 1,000-actor spawn median was 476.1 versus 478.7 ms. That small saving does
not justify permanent string-table entries and equality checks on every worker env overwrite.
The experiment was reverted; records are in `target/actor-env-values-measure.jsonl`. The
remaining actor cost is in the broader bootstrap graph and its allocator residency.

### 2026-09-29 -- later actor closure sources bound the closure-only saving

The opt-in `KILN_HEAP_PROFILE=1` now groups live closures by their emitted source at the first
two actor turn boundaries. On the public `actor-capacity` native-release binary, the second
turn of a later actor held 181 closures with 40,544 bytes of GC payload; 131 captured upvalues.
The source breakdown was:

| Source | Closures | Capturing | Closure payload |
| --- | ---: | ---: | ---: |
| Runtime bootstrap | 82 | 62 | 18,368 B |
| `worker_threads` | 33 | 22 | 7,392 B |
| `events` | 32 | 21 | 7,168 B |
| Actor core, worker and public module | 29 | 23 | 6,496 B |
| Target module and `async_hooks` | 5 | 3 | 1,120 B |

Even sharing every closure header would remove at most about 40 KiB of live payload before
realm-local captures and allocator effects. The profile does not attribute the actor's 198
ordinary objects, 159 arrays, 195 cells, their side buffers or their occupied GC pages to
sources. It therefore cannot identify a safe single-module object subtree yet. The next
candidate must construct the actor-enabled bootstrap's objects and captured state directly
from shared defaults and show a public held-RSS change; a closure-only extension cannot meet
the tens-of-KiB target. The raw profile is in `target/bootstrap-source-actor-run.log`.
The backend suite passed 257 unit and 36 emitted-C tests; the four-actor capacity probe checked
all replies and its checksum.

### 2026-09-29 -- actor side-buffer capacity and resident-page bound

The opt-in turn-boundary heap profile now totals capacity owned outside GC blocks for ordinary
object property arrays, shaped slots, sealed values, object and array element runs, and closure
upvalues. It excludes process-wide shapes/keys, provider state, allocator overhead and other
GC kinds' side allocations, so it is an ownership inventory, not an RSS decomposition.

On the public `actor-capacity` native-release binary, three later actors had the same second-turn
sample: 140,625 bytes of GC payload and 44,924 bytes of counted side capacity. Of the latter,
35,424 bytes were ordinary object property arrays, 5,120 shaped slots, 1,012 sealed values,
1,024 object elements, 1,152 array elements and 1,192 closure upvalues. The three promoted
chunks had live blocks on 3, 38 and 30 complete 4 KiB body pages, respectively. Page reclamation
can release unoccupied pages but cannot pack these 71 occupied pages.

The same instrumented binary checked 100 and 1,000 held public actors with reply sums 5,050 and
500,500; held RSS was 83,992 and 622,460 KiB, an added slope of about 598 KiB per actor. A
four-actor run with every-allocation GC stress also completed with the expected checksum. The
counted 185,549 bytes of payload plus side capacity are not an upper bound on retained memory:
the much larger RSS slope includes occupied GC pages, slabs, other allocations and provider
state. A property-array-only retrofit has a 35 KiB capacity ceiling before allocator effects.
The first shared-default subtree must include the actor-enabled bootstrap objects and captured
state that keep those pages occupied, then show a same-workload RSS decrease; a local side-buffer
reduction alone cannot establish the density target. Raw samples are in
`target/actor-side-profile-{run,1000,stress}.log`.

A temporary first-key sample of the later actor's nine property arrays with capacity at least
eight accounted for 34,656 of the 35,424 property-capacity bytes. The two largest were the
realm-specific `process.env` store (256 records / 12,288 bytes in this environment) and the
wide global store (234 / 11,232 bytes); the next was a constructor-linked store (128 / 6,144
bytes). Other stores were at most 1,536 bytes each. The print instrumentation was removed after
the run. The environment store is user and host data, so sealing its particular keys process-wide
would be wrong. The global already has a post-bootstrap sealed descriptor; direct construction
there needs the full ordered overlay described in `bootstrap-object-layout.md`. Extending the
existing two-store direct writer to another isolated callable cannot save enough capacity to
close the public actor gap. The temporary run is in `target/actor-store-sample-run.log`.

### 2026-09-29 -- reclaim pages with no live nursery block

After a nursery chunk is promoted and settled, its bitmap identifies every surviving block.
The runtime now counts the complete body pages intersected by each live block, including the
block header and every page of an interior payload, and asks `madvise(MADV_DONTNEED)` to release
only the remaining complete pages. The chunk header and bitmaps remain resident, and a failed
advice is harmless. Reclamation repeats only when a later collection removes more blocks.
An opt-in page profile on later public actors found 39 and 31 occupied complete pages out of
61 in each of two full promoted chunks, with a small third chunk. This made page reclamation
a measurable intermediate capacity step even though it cannot reach the tens-of-KiB target.

On one native-release binary with a forced low GC floor, four interleaved rounds at 1,000 held
actors measured 843,008–843,592 KiB without reclamation and 630,660–630,960 KiB with it; all
replied with checksum 500,500. At 100 actors, the corresponding ranges were 105,868–106,004
and 84,184–84,764 KiB, with checksum 5,050. Three further interleaved 1,000-actor rounds with
the normal GC setting measured 842,776–843,252 KiB without reclamation and 622,712–622,812 KiB
with it. The final default-on binary held 622,520 KiB at 1,000 actors and checked all replies.
The opt-in rounds showed about a 3–4% spawn-time cost at 1,000 actors; these are same-host
comparisons, not a broad performance baseline. Raw rows are under
`target/actor-capacity-reclaim-results.jsonl` and the focused run logs under `target/`.

The backend C suite passed 255 tests with one ignored and 36 emitted-C cases. A focused fixture
kept a 9,000-byte string spanning pages live through two collections while dead-only pages
were eligible for release; it also passed with AddressSanitizer. Stressed native realm probes
for deep JSON and prepared actor transfer matched at 1, 2 and 8 realms, and the normal realistic
direct actor graph matched at those counts. Five focused interpreter/native parity probes
matched; four matched again under every-allocation collector stress. The small direct-move
rollback probe matched at 1, 2 and 8 stressed realms. The full `scripts/native.sh` was attempted
and stopped at `app-bun-serve-global` before comparing native output: this sandbox cannot
complete its interpreter listener, which reports an unsettled top-level await. The preserved
log is `target/native-reclaim-full.log`; the full corpus and final native-stress gate remain
due in an environment with loopback sockets. The remaining roughly 600 KiB per public actor
still requires a shared bootstrap representation, including the Node worker and actor module
graph.

### 2026-09-29 -- smaller nursery chunks do not lower the actor slope

The current native-release `actor-capacity` binary checked all replies while holding 100 and
1,000 public actors. Two interleaved 1,000-actor runs at the default setting held 842,848 and
842,720 KiB; `KILN_NURSERY_KIB=128` held 878,772 and 879,392 KiB; and
`KILN_NURSERY_KIB=64` held 842,748 and 842,428 KiB. All used 18 threads and returned checksum
500,500. The corresponding 100-actor held RSS values were 105,576, 108,416 and 104,608 KiB;
the 100-to-1,000 slope is therefore about 819, 856 and 820 KiB per actor. A 256 KiB budget
held 841,852 KiB at 1,000 actors and 104,908 KiB at 100, likewise little different from
default. Spawn and call times stayed within a few percent in these short runs. Reducing the
chunk size alone does not free the retained pages of the public actor bootstrap; 128 KiB is
worse on this workload. Keep the default and pursue the shared bootstrap representation.

### 2026-09-29 -- limiting glibc arenas does little for public actor density

The current native-release public `actor-capacity` binary kept 100 and 1,000 actors alive
while recording RSS and the checked reply checksum. At 100 actors, default glibc held
105,700 KiB versus 105,156 KiB with `MALLOC_ARENA_MAX=1`. Two interleaved 1,000-actor
runs held 843,048/843,320 KiB by default and 838,320/836,924 KiB with one arena. The
approximate 100-to-1,000 slope changes from 819 to 814 KiB per actor: a ~5 KiB saving,
far below the density target. The one-arena 1,000-actor runs took about 699–700 ms to
spawn and 127–128 ms to stop, versus 570–574 ms and 60–62 ms by default. These few
same-host runs do not establish a stable throughput ratio, but they rule out arena count
as the main retained-memory cause. Keep the normal allocator setting; the measured live
bootstrap graph and occupied GC pages remain the relevant work.

### 2026-09-29 -- late closure sharing has a small bound

The opt-in turn-boundary heap profile now classifies native closures by captured upvalues and
arrow form. In a 100-actor public capacity run, three later actors had the same second-boundary
breakdown: 131 closures with upvalues, 22 arrows without upvalues, and 28 ordinary closures
without upvalues, out of 181 closures / about 40.5 KiB of closure payload. All had generated
source rather than a runtime-created source value. The run kept 100 actors, checked the 5,050
reply checksum and held 18 threads at 105.7 MiB RSS.

Even if all 28 ordinary uncaptured closures could use the sealed process region, their 224-byte
headers total only about 6.1 KiB per actor before allocator effects. Arrow lexical state and
the 131 captured environments need a separate proof; upvalue count alone is not enough to share
them. Extending the existing once-made region closure mechanism to a few late module sites is
therefore not the primary route to public actor density. The retained Node/actor bootstrap
objects, captured cells and allocation-page residency must be addressed together. The profile
is an opportunity bound, not an eligibility certificate for any particular closure site.

### 2026-09-29 -- actor turn-boundary retained heap by kind and chunk

An opt-in `KILN_HEAP_PROFILE=1` diagnostic samples the first four realm heaps at their first two
turn boundaries. It walks the live block indexes without collecting and prints kind totals and
promoted-chunk occupancy. On the public `actor-capacity` native-release program, each of three
later actor realms had about **1,399 blocks / 140.5 KiB payload** after its second boundary, with
198 objects (~41 KiB), 181 closures (~40.5 KiB), 159 arrays (~26 KiB), and 587 strings (~22.7
KiB). The counts include the actor's module and constructor, unlike the earlier
bootstrap-complete snapshot. The printed totals matched the heap's own count and byte totals.

Each later actor retained three promoted nursery chunks. One had 609 live blocks / 70.6 KiB
payload from a 255.8 KiB allocation span; another had 402 / 51.0 KiB from 255.9 KiB; the third
had 75 / 7.6 KiB from a 15.2 KiB span. Three lazily backed slabs and an active chunk remained.
The 100-actor capacity run completed with the expected checksum, 18 threads and 105.7 MiB held
RSS. These are one-run allocation diagnostics, not an RSS attribution for every byte.

The two full chunks contain far more dead than live storage, so a typed packed bootstrap image
has a plausible one-chunk saving on this complete public actor path as well as the measured
1.6–1.8 ms repeated TypeScript bootstrap phase. The third chunk's small used span should not be
counted as a fully resident 256 KiB. An image remains a startup and partial-capacity step; only
the shared-default representation can approach the tens-of-KiB target.

### 2026-09-28 -- public actor bootstrap keeps two nursery chunks alive

A temporary diagnostic at `kiln_c99_builtin_bootstrap_done` counted the heap's nursery chunks
and slab slots in the public `actor-capacity` native-release program. A forced collection at
that point left each later actor with 1,050 GC blocks and about 104.5 KiB of live payload. The
live nursery blocks occupied **two 256 KiB promoted chunks**: 608 blocks / 70,403 payload bytes
in one, and 152 blocks / 23,970 bytes in the other. Three 64 KiB slab address ranges held 290
live slots (14 of size 96, 124 of size 80, 152 of size 64). The first, shared-region-building
realm retained one promoted chunk. Without that forced collection, a later realm had one full
and one active nursery chunk, 3,530 blocks and about 361 KiB allocated payload at the same
point. The ten-actor program returned the expected checksum with and without collection.

The 64 KiB slab ranges are lazily backed, so their virtual sizes are not a direct RSS charge.
The promoted nursery chunks, however, each hold live blocks and cannot be released by the
current non-moving collector. This explains why lowering the collection threshold did not cut
the measured 816 KiB/actor RSS slope: it discards dead blocks inside retained chunks. A compact
copied bootstrap image could plausibly pack the roughly 94 KiB of live nursery payload into one
chunk and improve startup, but needs the typed root/edge relocation proof before claiming a
256 KiB saving. The shared-default representation remains the density end state. Probe logs are
under `~/.cache/kiln/actor-density-2026-09-28/`; the diagnostic code was removed.

### 2026-09-28 -- one C realm-root inventory for collection and graph proof

The realm context's strong C roots now go through one read-only enumerator, with distinct
`kiln_value` and compact generated-unit `kiln_val` slots. Both collector marking and the
direct-transfer incoming-alias check use it; the latter can exclude only the active escrow slot.
Provider and generated launcher roots already have read-only visitors. Promise jobs, weak
key-table pointers and other C side lists retain their separate ownership checks. The source
inventory and the remaining reachable-edge audit are in `../bootstrap-image-design.md`. This
refactor establishes a shared root boundary; it does not yet publish a bootstrap blueprint or
reduce realm memory.

The backend unit cases passed 253/253 and the emitted-C snapshot cases 36/36. Ordinary native
parity passed 349 matches, one known refusal and zero divergences; full collector stress passed
309 matches, one known refusal and zero divergences (40 declared stress skips). Three focused
probes matched at 1, 2 and 8 realms with collector stress. A final native parity rerun after the
interpreter stack-guard fix again passed 349 matches, one refusal and zero divergences.

### 2026-09-28 -- GC threshold is not the public-actor memory lever

On the same `native-release` public actor-capacity binary, six alternating runs each at 100 and
1,000 live actors compared `KILN_GC_FLOOR_KIB=000` with `256` (equal-length values in the realm's
`process.env`). At 1,000 actors, both settings held 838-839 MiB RSS, used 18 threads, and took
about 384-396 ms to spawn and 185-189 ms to call all actors. The aggressive threshold did not
change the retained-memory slope or work times materially. This reinforces the retained
worker/module graph diagnosis; a bootstrap collection is not the implementation of issue 08.
The representation and fallback gates are in `../bootstrap-image-design.md`.

### 2026-09-28 -- public actors retain much more than the minimal core realm

The new public actor capacity workload keeps a linked `Ping` actor module and mailbox alive in
each realm. At 100 and 1,000 actors, held native-release RSS was about 103 and 819 MiB with 18
threads, an added slope of roughly 816 KiB/actor. A minimal scheduled `node:worker_threads`
worker kept through the same bounded lanes used 749 MiB for 1,000; the actor wrapper accounts
for roughly another 70 MiB, while the common worker/module bootstrap dominates. These are
process RSS measurements, not a retained-heap inventory or a claim that each realm requested
816 KiB of live payload.

A symbol-retaining `native-profile` build under jemalloc's sampled heap profiler attributed the
large allocation classes to `kiln_c99_nursery_new_region` and `kiln_c99_gc_slab_new`. Its sampled
byte estimates exceed RSS and are not used as absolute allocation counts. At 1,000 actors,
`MALLOC_ARENA_MAX=1/2/16` changed held RSS by only about 3 MiB while lower arena counts slowed
spawn; jemalloc was likewise within a few MiB. A 64 KiB nursery chunk saved about 13 MiB across
1,000 actors and slowed the run, while 128 KiB was worse and changing only the collection budget
did not help. Stressed collections at 100 actors increased both time and RSS. Allocator settings,
smaller chunks and forced collection therefore cannot close the public actor density gap.

The next representation must cover the retained Node worker and actor module bootstrap graph as
well as the core built-ins. It needs shared immutable code/descriptors with realm-owned values,
identities and mutation, and a GC/read-path audit for every direct object consumer. Measure the
public actor slope again after that change; the minimal Date-plus-timer slope alone does not
establish actor density. Raw measurements and profile summary are under
`~/.cache/kiln/actor-capacity-2026-09-28/`.

### 2026-09-28 -- sealed-store conversion cursor cuts warm actor startup

A native-profile run of 1,000 public actor spawns attributed 0.48 seconds of self time to
`kiln_c99_sealed_property_nth`, called 117,117 times by 10,687 conversions of a sealed store
to a dictionary. Each conversion restarted the insertion-order search for every property.
Conversion now carries the previous order into the next selection, preserving the same
descriptor and realm-owned value path while avoiding that repeated prefix walk.

Seven alternating 100-actor rounds on native-release binaries from the same source measured
warm `spawnActor` medians of 2.697 ms before and 2.142 ms after (matching checksums in all
rounds). Backend-C tests passed 253 unit cases and 36 emitted-C cases; native parity reported
345 matches, one known refusal and zero divergences. Two sealed/global probes matched at 1, 2
and 8 realms. Full native collector stress reported 306 matches, one known refusal, zero
divergences and 39 declared skips. The profile, raw comparisons and logs are under
`~/.cache/kiln/actor-startup-profile-2026-09-28/`. This removes one repeated-bootstrap cost;
the typed bootstrap image remains the structural startup step, and the added-realm memory
target remains open.

### 2026-09-28 -- assign directly into a sealed realm value slot

The actor-start native profile found an avoidable bootstrap cost: assigning an existing property
of a sealed store materialized the entire process descriptor into a realm dictionary first. The
storage boundary now writes a present, writable sealed data property into that realm's value
slot. Read-only assignment retains its strict-mode error; new keys, accessors and descriptor
changes still take their ordinary storage path. The assignment leaves insertion order and the
shared descriptor untouched.

Seven interleaved 100-actor rounds on one native-release build per side measured the warm public
`spawnActor` median at 2.70 ms before and 2.60 ms after, with every round's checksums intact.
The prior per-realm TypeScript bootstrap took 1.6–1.8 ms, so this removes only a small part of
repeat work. The bootstrap image remains the structural startup step. The backend crate's 245
unit and 36 emitted-C tests passed; native parity reported 343 matches, one known refusal and
zero divergences. The focused assignment probe matched under collector stress and at 1, 2 and
16 realms, plus stressed 1, 2 and 8 realms. The 10-step routine gate passed (2,272 crate tests,
97,110 routine Test262 passes with no chunk regressions); full native GC stress reported 304
matches, one known refusal, zero divergences and 39 declared skips. Logs are preserved under
`~/.cache/kiln/sealed-assignment-2026-09-28/`.

### 2026-09-28 -- retained bootstrap inventory for the startup image

Temporary instrumentation on the native-release public `actor-start` binary forced a collection
immediately before and after the TypeScript bootstrap, then counted live heap blocks. It was
removed after measurement. The first actor realm grew from 468 blocks / 22,367 payload bytes to
1,014 blocks / 100,705 bytes. A later realm grew from the same pre-bootstrap inventory to
1,049 blocks / 104,670 bytes: **581 added live blocks / 82,303 payload bytes**. The later-realm
delta includes 154 objects, 139 arrays, 79 realm-local closures, 78 strings, 65 cells, 34 method
stores, 10 collections and smaller kinds. There are 30 separately owned object slot arrays and
2 array side buffers after bootstrap. The raw diagnostic is in
`~/.cache/kiln/bootstrap-inventory-2026-09-28/bootstrap-inventory-run2.err`.

This is a reachable-heap inventory, not a ready-to-copy image: the GC walks the whole realm on
each side, and the difference does not identify the mutations to pre-existing native roots or
prove which fields point to realm-local state. It sets the scale of the relocation work. The
startup image needs a typed pointer/host-binding inventory and the fallback and stress gates in
`../bootstrap-image-design.md` before it can replace the per-realm bootstrap.

### 2026-09-28 -- actor startup phase profile

The public actor benchmark added under issue 13 measures a 2.80 ms warm `spawnActor` to ready
median. Temporary timing probes (removed after profiling) split that path on a native-release
binary: preparing the call took about 0.03 ms, constructing the Node-compatible `Worker` wrapper
about 0.2 ms, actor registration under 0.01 ms, and the wait for ready about 2.55–2.75 ms. The
scheduled actor's first language turn began within about 0.1 ms of its native spawn request, then
took about 2.4–2.7 ms. Inside that turn, native-module installation took 0.16–0.19 ms and the
per-realm TypeScript bootstrap took 1.6–1.8 ms; the target actor module import and constructor
together took under 0.2 ms. The first actor pays additional one-time setup. Trace logs are in
`~/.cache/kiln/actor-start-2026-09-28/profile/`. These few probes locate the dominant phase;
the three-round benchmark in issue 13 supplies the representative end-to-end number.

Startup therefore needs a shared *bootstrap object template*, not just the existing shared
closures. The first cut should copy and relocate its mutable object headers, slots and cells into
each realm rather than put copy-on-write branching into every property read. The plan and proof
gates are in `../bootstrap-image-design.md`. Measure the public actor start benchmark and the
single-realm hot paths before enabling it by default. This is startup work; immutable backing
and copy-on-write for the remaining tens-of-KB memory target can follow after the image is
correct. A prewarmed pool would hide the cost while retaining idle realms, so it is not the first
implementation step.

### 2026-09-27 -- first cut (branch `work/shared-region`, commit d044f96f)

**Where a second realm's memory went**, measured before changing anything (origin/main 3ffad3e4,
native-release, `hold` = `console.log("hi"); setTimeout(() => {}, 1500)`, run under `env -i` so
`process.env` is empty, anonymous memory from `/proc/<pid>/smaps_rollup`, median of 5, and an
`LD_PRELOAD` malloc tracker per thread): a second realm (`KILN_REALMS=2` minus `=1`) cost 688 KB.
Per realm:

- the nursery: the bootstrap allocates ~207-238 KB before its first collection, of which ~110 KB is
  live (1,223 blocks: 327 closures are ~73 KB of it) and the rest garbage; it fills the first 256 KiB
  chunk and spills into a second;
- key-copy slabs: two or three 64 KiB slabs, each written whole by `kiln_c99_gc_slab_new` (it
  threaded every slot onto the free list) for the handful of realm-local keys of one size class;
- realm-local shapes: 136 of them at 416 bytes, and ~110 realm-local keys, in every realm and in a
  lone one too. Every key the runtime spells as a C literal (`kiln_c99_put(NULL, o, "fd", 2, ...)`,
  the global object's `{"parseInt", ...}` table, `process`, `console`), every key the graph launcher
  spells for `process.__kilnNativeModules`, and the names the bootstrap writes as *values* (`const
  added = ["count", "countReset", ...]` then `defineProperty(console, added[i], ...)`) was interned
  at run time, and every shape built from one was the realm's own;
- the realm thread's stack (88 KB), Tokio, the launcher.

**What changed.**

1. *Keys.* build.rs scans `runtime/*.c` and the launcher (`kiln-native/src/graph.rs`) for the one
   way this runtime spells a key -- a literal followed by its exact length, or a `{"Name", KILN_C99_`
   table row -- and generates `KILN_C99_PRELUDE_LITERAL_KEYS`, which the prelude's load-time
   constructor registers beside the surface names (ADR 0071: built before the first realm). A unit
   emitted with `shared_region` (the bootstrap) also registers its name-like string constants. A
   realm's startup now makes **no** realm-local shape or key (`local_shapes=0 local_keys=0` in the
   SIGUSR1 heap line, which now reports both).
2. *Slabs grow on demand.* A slab hands out slots `[0, fresh)` and touches the next one only when it
   is needed; every walk and lookup stops at `fresh`. Nursery chunks already did (the bump touches
   pages as it goes), so the "small first chunk" issue 01 asked for comes to this: what is resident
   is what was allocated.
3. *The region* (`heap.c`, "The shared immutable region"): a page-aligned reservation (4 MiB of
   address space) in which the first realm to reach a region site builds; its
   `kiln_c99_builtin_bootstrap_done` makes the used pages read-only (`mprotect`) and publishes
   SEALED; later realms read it. A realm arriving mid-build waits (bounded, 250 ms: the builder is
   running one synchronous module body on another thread); a builder torn down unsealed abandons it;
   `KILN_REGION=0` keeps a realm out. The collector finds no region block in any heap table, so it
   never marks, traces or frees one. What the region holds so far: the bootstrap's top-level
   closures (`EmissionPolicy::shared_region`, set for the bootstrap alone): a closure-making site on
   no cycle of the module body, capturing nothing, and not an arrow that copies its frame, becomes
   `kiln_c99_region_closure`, one closure per process. The core bootstrap yields 38 (58 blocks with
   their names, 11 KB); a `--node` build 135 blocks, 25 KB.
4. *The no-write read path*, for parallel-builtins issue 06 (see its comment).

**The per-realm identity decision.** Node gives a worker (and a `vm` context) intrinsics of its own:
`Array` of one realm is not `Array` of another. What makes that observable is holding both at once,
and in kiln nothing can: realms exchange only structured clones and transferred buffers, `vm` runs in
the calling realm, and `ShadowRealm` is refused. kiln's C built-ins already rely on this -- `Array` is
`KILN_VALUE_BUILTIN` with a number, the same bits in every realm, and what a program can change about
it is its realm's own store (`kiln_realm_callable_store`). The region extends exactly that model to
closures: *identity* is shared, *every mutable part* is the realm's. A region closure's `prototype`
and property store are fields no path reads directly any more (renamed `own_prototype`/`own_store`;
a direct access does not compile) -- `kiln_c99_closure_{prototype,store}{,_slot}` answer them, and for
a region closure they live in the realm context's table, marked with the realm and freed with it.
ADR 0059 invariant 4 ("a realm creates distinct intrinsic object identities") holds for everything a
program can observe; if `vm` ever gets real contexts, a region closure would need a per-context wrapper.

Objects -- prototypes, class objects, the stores the bootstrap fills -- stay per realm, deliberately.
An object's properties are read inline by every cache hit the emitter writes (shape compare, slot
load); a shared object with a per-realm half needs a test on that path for every object, or a
one-way "forked" shape plus a forwarding lookup in every slow path that dereferences an object
(`kiln_c99_object_of` has 226 callers, and some slow paths write while reading -- `ensure_dictionary`,
layout tokens). A closure's mutable parts, by contrast, were already behind a handful of accessors.
So the region takes the behaviour of the bootstrap -- its method closures, names and code -- and each
realm keeps thin identity objects: this issue's "share the immutable backing, keep per-realm
identity", with the closures on the shared side because their identity is not observable.

**Why so few closures qualify yet.** The bootstrap allocates 327 closures; 67 closure-making sites are
in its top-level body, and 29 of those capture something. kiln compiles a module binding a function
reads as a *captured cell* (`kiln-ir`, `a_function_reads_module_scope_through_a_captured_cell`), so
`Uint8Array.prototype.toHex = function toHex() { ... __kilnUint8ToHex(this) }` captures the realm's
cell for `__kilnUint8ToHex`; and most methods are made inside `install*()` functions the top level
calls once. Both are compilation questions for the bootstrap, which is never imported: read its
top-level bindings from the realm's unit block (`kiln_mg`, as module globals already are) instead of
capturing cells, and treat a function the top level calls once, and nothing else references, as part
of the top level. With that, nearly every bootstrap closure is a region closure. That belongs with the
bootstrap-compilation work (kiln-native build orchestration), not here, and nothing on the region
side has to change for it.

**Measurements** (Linux x86-64, Ryzen 7 5800H, a shared machine at load 8-13; native-release;
`env -i`; anonymous private memory, median of 5, sampled 0.8 s after start; binaries built by
origin/main 3ffad3e4 and by this branch from the same sources):

| figure | origin/main | this branch |
| --- | --- | --- |
| `hold`, one realm | 1,488 KB | 1,416 KB |
| `hold`, `KILN_REALMS=1` / `=2` / `=4` | 1,524 / 2,212 / 3,584 KB | 1,452 / 1,924 / 2,868 KB |
| **a second realm** (`=2` minus `=1`) | **688 KB** | **472 KB** |
| each realm past the first at `=4` | 687 KB | 472 KB |
| `worker_threads` (`--node`), main alone / + 1 worker | 2,556 / 4,296 KB | 2,536 / 3,984 KB |
| **a worker** | **1,740 KB** | **1,448 KB** |
| each of 4 workers | 1,537 KB | 1,261 KB |
| `bench/server/serve.mjs`, one realm | 2,540 KB | 2,452 KB |
| realm-local shapes / keys at startup (`hold`) | 136 / ~110 | 0 / 0 |
| the region | -- | 58 blocks, 11 KB (`--node`: 135, 25 KB) |
| `hello`, spawn to exit, median of 400 (four alternated pairs) | 5.57 / 5.77 / 6.02 / 2.85 ms | 5.58 / 5.61 / 5.17 / 2.60 ms |
| `hello` binary | 11,442,416 B | 11,500,904 B |

Startup is within noise on this machine (the last pair was the only quiet one); the extra
registrations before `main` are paid back by the shapes and key copies the realm no longer makes.

**Verified** (once, on the final tree). `mbx test -p kiln-backend-c`: 192 passed, 1 ignored; emitted_c
36 (ABI surface snapshot re-recorded; the prelude size band widened to 55,600). New tests:
`the_shared_region_holds_one_closure_and_each_realm_keeps_its_own_parts` (builder and user heaps: one
closure, per-realm prototype and store, the name copied into the region, a collection with a realm
object holding the closure, the pages read-only after sealing, the no-write path writing nothing --
mutation-checked by disabling the no-write test) and
`a_region_whose_builder_is_torn_down_unsealed_is_abandoned`. `mbx test -p kiln-native`: 228 passed, 2
ignored. `scripts/native.sh`: 270 matched, 1 refused (shadow-realm), 0 diverged. `KILN_REALMS_COUNTS="1
2" scripts/native-realms.sh`: 265 matched, 5 skipped, 1 diverged -- `worker-threads-broadcast`, whose
`BroadcastChannel` reaches the other realm of the process; it diverges identically on origin/main.
`KILN_NATIVE_GC_STRESS=1 scripts/native.sh` over accessors, array-methods,
builtin-static-method-cache, classes-and-callables, collections, define-site-cache,
json-shaped-build-edges, negative-cache-invalidation, object-literal-shapes, intl-segmenter,
iterator-helpers and region-builtins-per-realm: 12 matched, 0 diverged. The full `mise run
native-stress` was not run.

The acceptance probe is `conformance/native/region-builtins-per-realm.cjs`: the main realm writes
`Object.groupBy.marker`, `Intl.Segmenter.marker` and `Intl.Segmenter.prototype.extra` and makes
`Map.groupBy` non-extensible -- all region closures -- then a worker reports none of it, and the main
realm still sees all of it after; node, the interpreter and native agree, with and without the region.

**Acceptance.**
- A second realm drops by 216 KB, more than the bootstrap's live heap (~110 KB) -- but mostly through
  the keys and slabs above, not by sharing the bootstrap: the region takes 11 KB of it today. The
  bootstrap still runs in every realm and its objects are every realm's own (by the decision above);
  that is the remaining work, not done.
- Modifying a built-in in one realm is invisible to every other: the probe above.
- native-stress: the short subset above is clean; the full run is outstanding.

**Found, not fixed (pre-existing on origin/main).** `hold` built by origin/main segfaulted 3 times in
~90 runs of `KILN_REALMS=4` (two threads inside the runtime near a malloc, stripped binary, not
root-caused); this branch's binary did not crash in ~140 runs, which says nothing certain about why.

**Left / risks.**
- The region's economy is small until the bootstrap's closures stop capturing cells (above).
- The bootstrap's objects and its allocation volume (~207 KB per realm, about half garbage by the
  first collection) are what a realm still pays for at start; the ADR's tens of kilobytes needs the
  bootstrap not to run per realm, which needs a way to share objects (the decision above says why that
  is expensive) or to make the bootstrap lazy per member.
- A realm reaching a region site while the builder never finishes its bootstrap waits 250 ms and then
  makes its own closures; only the graph launcher runs the bootstrap, and it seals on success.
- `KILN_REGION=0` is the escape hatch; a region closure is never written after sealing, so a path
  that forgets the accessors faults at the write (read-only pages) instead of leaking a change into
  another realm.

### 2026-09-27 -- second cut: the bootstrap's closures capture nothing (branch `work/region-fill`, dd4d36e1)

**Design.** The choice was between restructuring the bootstrap's emission (hoisting closure creation
out of `install*()` to the top level) and letting a closure reach the cells it would have captured
by the realm it runs in. Hoisting changes what the source means (an install function's locals are
per call; hoisted closures would need them as module bindings, i.e. a source transform, and every
`const at = { at() {} }.at` shape to follow), so the second: `kiln-backend-c/src/module_cells.rs`, a
rewrite of the unit's `Program` run only under `EmissionPolicy::shared_region`.

- A *once body* is the top level, or a function a once body makes on no cycle and uses only as the
  callee of one call on no cycle (a dead store of it to an unread slot -- a named IIFE's own binding
  -- allowed). It runs at most once per realm. The bootstrap has 12 besides the top level: every
  `(function install*() { ... })()`.
- A cell *moves* when a once body makes it with its only `NewCell` for that slot, on no cycle, never
  uses the slot as a plain local, and some function captures it. The top level's cells already live
  in the realm's unit block (a unit with module globals keeps its whole top-level frame there); a
  once function's cell is *published* to a new block slot past the top level's, by the emitter,
  right after `MakeCell` (`kiln_mg[G] = locals[t]`). A `let` in a loop keeps its per-iteration
  capture.
- Every function's `LoadUpvalue`/`StoreUpvalue` of a moved cell becomes `LoadGlobal`/`StoreGlobal` of
  its block slot and the upvalue is dropped (the rest renumbered, `ParentUpvalue` chains remapped);
  the emitter lowers a `LoadGlobal` of a cell slot to a read through the cell with the same dead-zone
  check an upvalue read has (unchecked where `direct.rs` proved the binding settled), and a store to
  a write through it. The binding keeps its cell -- dead zone, writes, one box for every closure --
  and a function finds the box through the generation-checked unit cache every module global uses.
- Region sites are the closure-making operations of the once bodies, on no cycle, of functions that
  capture nothing after the rewrite (and not frame-copying arrows), keyed by function index -- each
  function has one creation site. Chunked (specialized release) units reach them through
  `{sym}_make_closure_region`, next to the ordinary helper; `{sym}_region_sites` is declared once per
  unit and `extern` in chunks.
- `direct.rs` follows a published block slot back to the function and local it came from, and no
  longer reads a cell global as a raw value, so direct calls and dead-zone proofs are unchanged.

Per-realm identity is exactly the first cut's decision: the closure is shared; its `prototype`,
property store and now every binding it reads are the realm's. A realm with `KILN_REGION=0` makes
ordinary closures that still read the block, so the escape hatch still works.

**What qualifies.** Core bootstrap: 363 functions, 646 captures before, 82 after; 331 capture
nothing, and all 327 closure-making sites in once bodies are region sites (was 38). What still
captures (32 functions) closes over a local of a function that runs per call (`JSON`'s
`internalize`/`serialize` helpers, callbacks built inside methods), which is right: those closures
are made per call. Region: 371 blocks / 86 KB core, 998 blocks / 203 KB `--node` (was 58 / 11 KB and
135 / 25 KB).

**Measured** (Linux x86-64, Ryzen 7 5800H, shared machine at load 1-17; native-release binaries built
by origin/main 9ef3069b and by this branch from the same sources; `env -i`; anonymous memory from
`smaps_rollup`, median of 5, 0.8 s after start; programs as in the first cut):

| figure | origin/main (first cut) | this branch |
| --- | --- | --- |
| `hold`, alone / `KILN_REALMS=1` / `=2` / `=4` | 1,364 / 1,436 / 1,876 / 2,756 KB | 1,368 / 1,432 / 1,788 / 2,500 KB |
| **a second realm** (`=2` minus `=1`) | **440 KB** | **356 KB** |
| each realm past the first at `=4` | 440 KB | 356 KB |
| `worker_threads` (`--node`), main alone / +1 / +4 workers | 2,444 / 3,676 / 6,672 KB | 2,460 / 3,540 / 6,060 KB |
| **a worker** | **1,232 KB** | **1,080 KB** |
| each of 4 workers | 1,057 KB | 900 KB |
| bootstrap allocation per realm (SIGUSR1, before any collection) | 207 KB / 1,310 blocks | 142 KB / 1,022 blocks |
| `hello`, spawn to exit, median of 400, four alternated rounds | 2.20 / 2.15 / 2.29 / 2.26 ms | 2.08 / 2.18 / 2.17 / 2.07 ms |
| `hello` anonymous memory (one realm) | 1,360 KB | 1,368 KB |
| `hello` binary | 11,026,472 B | 11,051,032 B |
| a bootstrap-method loop (`at`, `toHex`, Date getters, `subarray().indexOf`, `toSorted`), 5 alternated runs | 2,050-2,094 ms | 2,040-2,077 ms |

(origin/main measured 440 KB for a second realm on this day, not the 472 KB the first cut recorded
on a busier machine; the two columns are one session's.) The loop row caught a 7% regression in an
earlier state of the change -- every rewritten read took the checked path -- fixed by honouring
`direct.rs`'s settled-binding proof.

**Where the 356 KB goes now** (from `smaps` of `=1` against `=2`): the realm's nursery pages touched
by the bootstrap's 142 KB of allocation (of which ~40-65 KB is live after a collection; the rest is
descriptors, arrays of names and store growth, garbage), about 100 KB of the realm thread's malloc
arena (object slot arrays, dictionaries, the realm context), the realm thread's 88 KB stack, and
the rest small. The target of tens of kilobytes is not reached, and closures are no longer where
the memory is: what is left is the bootstrap's *objects* and its garbage, which the first cut's
decision keeps per realm, and the thread. Levers, none taken here: collect once after the bootstrap
and return the nursery pages (the garbage half); have the TypeScript prelude stop allocating a
descriptor per `defineMethod`; a smaller realm-thread stack reservation; and eventually a lazy,
per-member bootstrap.

**Verified** (once, on the final tree). `mbx test -p kiln-backend-c`: 215 passed, 1 ignored (+36
emitted_c). `mbx test -p kiln-native`: 234 passed, 2 ignored, including two new tests:
`a_shared_region_unit_reads_once_made_cells_from_the_realm_block` (a top-level function, an install
IIFE and a method it makes are region sites, the method reading its install's local through the
block; a closure over a loop `let` keeps its capture; nothing changes without the policy) and
`the_core_bootstraps_closures_are_region_closures`. `scripts/native.sh`: 310 matched, 1 refused
(shadow-realm), 0 diverged. `KILN_REALMS_COUNTS="1 2" scripts/native-realms.sh`: 304 matched, 6
skipped, 1 diverged -- `worker-threads-broadcast`, the pre-existing issue 25. `KILN_NATIVE_GC_STRESS=1
scripts/native.sh` over accessors, array-methods, builtin-static-method-cache, buffer-prototype,
classes-and-callables, closures-in-loops, collections, dates, define-site-cache, errors,
intl-segmenter, iterator-helpers, json-shaped-build-edges, negative-cache-invalidation,
object-literal-shapes, promises, regexps, region-builtins-per-realm, structured-clone-dictionary-source,
typed-arrays, typed-callbacks, url-surface: 22 matched, 0 diverged. `KILN_NATIVE_GC_STRESS=1
KILN_REALMS_COUNTS=2 scripts/native-realms.sh` over typed-arrays, dates, classes-and-callables,
iterator-helpers, region-builtins-per-realm: 5 matched. The full `mise run native-stress` was not
run.

**Issue 07's comparison, re-run briefly** (`bench/server/serve.mjs --node`, idle, 1.5 s after start,
PSS / anonymous summed over the process tree, median of 3, no load): one realm 12.7 / 2.35 MB before,
13.2 / 2.38 MB after; `KILN_WORKERS=4` process mode 18.6 / 10.2 MB before, 18.9 / 10.2 MB after;
thread mode 16.0 / 5.3 MB before, 16.1 / 4.8 MB after. A thread-mode worker is now ~610 KB of
anonymous memory against ~730 KB before and ~1.95 MB for a forked one; thread mode still saves about
0.7 MB PSS per worker, as issue 07 found. This change widens the gap by ~120 KB a worker and does
nothing to throughput, which is what kept process mode the default (8% behind at 16 connections,
higher p99 at 64, no crash isolation). Memory alone does not argue for flipping
`KILN_CLI_DEFAULT_WORKERS_THREADS` yet; the owner's call stands where issue 07 left it.

**Left / risks.**
- The second realm is 356 KB, not tens of KB; see "where it goes" above.
- `LoadGlobal` of a cell slot is dead-code-eliminated like any `LoadGlobal`, dropping a dead-zone
  check an unused upvalue read would have kept, and a pre-suspend `LoadGlobal` may be placed past a
  call; both are unobservable for the bootstrap, whose reads never hit a dead zone (a bootstrap that
  did would fail every realm's start), and the rewrite runs for no other unit.
- A bootstrap method reading module state now finds the realm's block through the per-thread
  generation cache on entry (one TLS load and compare); the loop above shows no cost.
- Parallel clones refuse a body that reads the block (`ReadsGlobal`), so a bootstrap function that
  read a captured module constant is no longer a parallel-clone candidate; none of the parallel
  family's own paths depend on that, but it is a narrowing.
- The once-body analysis trusts that a closure used only as one call's callee cannot be called again;
  that holds for strict code (no `arguments.callee`, no `caller`), which the bootstrap is.

### 2026-09-27 -- memory follow-up (`work/realm-memory`, four implementation commits)

The bootstrap no longer constructs a fresh descriptor for each method installed by its helpers.
It reuses one descriptor per helper, and the error constructor loop no longer makes a string-key
array for each constructor. The compiler recognizes `{ m() {} }.m` when the literal has no other
use, so those install helpers also stop allocating a throwaway object and home cell for every
method. The emitter uses one site-streak table per thread rather than per unit. Once the
bootstrap has returned, the launcher gives the pages below its frame back to Linux with
`MADV_DONTNEED`; the next deep call gets fresh pages as before.

**Measured.** The same `hold` program and anonymous-memory method as the second cut, on this
branch's native-release binary, with `KILN_REALMS=1,2,4`, `env -i`, and five samples per count,
give 1,208 / 1,420 / 1,844 KB (median): about **212 KB per extra realm**. This is a 144 KB
reduction from the second cut's 356 KB per realm, and a 476 KB reduction from the first-cut
starting point of 688 KB. These are different measurement sessions, so the comparison is the
size of the effect rather than a precision benchmark.
Time from process spawn to the first `hold` line is 2.114 ms (median of 400), within the
second cut's startup range; the microsecond realm-start goal remains open.

An attempted forced collection at the bootstrap boundary was removed: in the same binary it
raised the second realm from about 216 to 232 KB. Live bootstrap objects hold the nursery's
pages, and the collection needs its own work storage. `KILN_NURSERY_KIB=128` and `=64` changed
the second-realm measurement from 212 KB to 204 and 208 KB, respectively, too small an economy
to change the collector's chunk size for every workload. `MALLOC_ARENA_MAX=1` lowered the
process's fixed cost but did not materially lower the per-realm slope.

**Checks.** `mbx test -p kiln-backend-c --offline`: 215 passed, 1 ignored, plus 36 emitted-C
snapshot tests passed. A full `scripts/native.sh` during the follow-up: 311 matched, 1 refused,
0 diverged. On the final binary, the five affected probes (`region-builtins-per-realm`,
`lone-method-literal`, `deep-recursion`, `iterator-helpers`, `errors`) all matched. The full
collector-stressed corpus is left to the final program gate.

The tens-of-KB target is still open. A pinned realm retains a thread's stack and allocator
arena, and its mutable built-in objects remain in its own heap. Issue 09's turn scheduler removes
the dedicated-thread cost; reducing the object cost needs lazy materialisation of built-in
members, or another shared representation whose read path preserves each realm's mutable
properties. The first cut's analysis of inline object reads rules out sharing those objects
without that design work. This follow-up does not change the process-mode default: issue 07's
throughput and crash-isolation findings still apply.

### 2026-09-27 -- residual-cost audit, before a further object-sharing design

The memory follow-up's 212 KB RSS slope is not 212 KB of live built-in objects. A read-only
inspection of its `hold` binary (`console.log("hi"); setTimeout(() => {}, 1500)`, Linux x86-64)
used `KILN_REALMS=1,2,4`, a minimal environment, `/proc/<pid>/smaps_rollup` at 0.5 seconds,
and five samples per count. With collection disabled only for the measurement, anonymous memory
was 1,208 / 1,420 / 1,844 KB: **212 KB per extra realm**, reproducing the final branch's
ordinary run. A `SIGUSR1` snapshot of the same idle realm reported **40,647 GC bytes / 518
blocks allocated** before any collection. The branch's experimental forced collection left
**33,221 bytes / 385 blocks**, freeing 7,385 bytes. That experiment had raised the RSS slope to
232 KB and was removed; its mark/work storage cost more than it reclaimed. The source of this
inspection differs from the final branch only in that disabled forced-collection call.

Controls on that same binary put the remaining cost in context:

| configuration | anonymous KB, 1 / 2 / 4 realms | extra realm KB |
| --- | --- | --- |
| ordinary nursery, GC disabled | 1,208 / 1,420 / 1,844 | 212 |
| nursery disabled, GC disabled | 1,312 / 1,620 / 2,236 | 308 |
| `MALLOC_ARENA_MAX=1`, GC disabled | 1,168 / 1,384 / 1,796 | 216 |
| `MALLOC_ARENA_MAX=2`, GC disabled | 1,208 / 1,376 / 1,800 | 168 |

The nursery is saving memory, not causing the 212 KB slope. The arena control shows sensitivity
to thread allocation, but its non-monotone results and possible allocator contention do not
justify setting a process-wide glibc policy for this issue. Page release after a forced
collection would have only 7.4 KB of dead payload to work with; the previous chunk-size
control moved RSS by just 4-8 KB, and the forced pass increased resident memory here.

**Design decision for review:** defer another shared-object or lazy-member representation until
issue 09 removes the pinned thread per realm and measures the residual slope. The entire
current realm's GC heap totals only 40.6 KB before collection, including its
ordinary mutable properties, and 33.2 KB after. A shared object with copy-on-write properties
would add a realm check or forwarding path to the numerous inline object reads described in the
first cut, for a maximum saving smaller than those totals. Lazy installation would similarly
need an exact materialisation hook on property reads, reflection and writes. The region already
shares the bootstrap's 86.7 KB of immutable closures. After issue 09, measure the same `hold`
program, `--node` worker startup, and a warmed small server; only design the remaining object
layer if those measurements show a material gap. Issue 08 remains in progress until that
measurement and the final native-stress gate.

### 2026-09-27 -- residual memory after the opt-in turn scheduler

The native-release `hold` program was remeasured in an owned worktree after issue 09's scheduled
launcher and turn migration landed. At 0.5 seconds, with a 1.5-second timer, two samples each,
`KILN_PARALLELISM=2`, and realms 1 / 2 / 4 / 16 / 100, scheduled anonymous memory was
**1,336 / 1,610 / 2,140 / 5,288 / 27,384 KiB**, with four process threads throughout. The
same binary's legacy dedicated-thread mode used **1,332 / 1,634 / 2,254 / 5,902 / 31,390 KiB**
and 2 / 3 / 5 / 17 / 101 threads. The 1-to-100 slopes are about **263 KiB** scheduled and
**304 KiB** legacy per extra realm. Scheduling removes the thread-count cost but does not meet
the tens-of-KiB memory target.

Controls at 1 and 100 scheduled realms left the slope near 263 KiB: `MALLOC_ARENA_MAX=1` or
`=2`, `MALLOC_TRIM_THRESHOLD_=0`, and `MALLOC_MMAP_THRESHOLD_=4096`. A 64-KiB nursery raised
it slightly to 267 KiB. A same-process `mallinfo2` snapshot put live glibc arena allocation at
432 KiB for one realm and 13,004 KiB for 100, about 127 KiB per added realm; it also reported
roughly four mmap allocations per realm. The remaining resident cost needs allocation-owner
inspection before selecting a shared-object or lazy-member design. These were short diagnostic
runs while other agents were building, not a quiet-machine performance baseline.

### 2026-09-27 -- data-property layouts and the residual (`work/memory-final`)

The first allocation-owner profile after the scheduler found a specific avoidable cost:
`Object.defineProperty` turned each shaped built-in store into a dictionary even when adding a
new data property. The runtime now keeps such definitions shaped when the object is extensible;
the shared shape records the descriptor's attributes, and the realm keeps the value in its inline
slots. Existing properties, accessors, and layouts beyond the shape's 32-property limit retain
the full dictionary path. The realm-local key table now starts at 64 entries rather than 256 and
grows at the same half-full threshold; the `hold` program uses 16 local keys.

An exact before/after comparison on `feba4345`, in one owned worktree, used the same
`console.log("hi"); setTimeout(() => {}, 1500)` source, `native-release`, a minimal environment,
`KILN_REALM_SCHEDULER=1`, `KILN_PARALLELISM=2`, `/proc/<pid>/smaps_rollup` anonymous memory at
0.55 seconds, and five runs per cell. These rows isolate the shaped-definition change; the key
table change came later.

| bootstrap | before, 1 / 2 / 16 / 100 realms (KiB) | shaped, 1 / 2 / 16 / 100 realms (KiB) | added-realm slope, 1 to 100 |
| --- | --- | --- | --- |
| core | 1,276 / 1,476 / 4,132 / 20,088 | 1,320 / 1,504 / 3,912 / 18,380 | 190 -> 172 KiB |
| `--node` | 1,928 / 2,336 / 7,780 / 40,432 | 1,988 / 2,376 / 7,604 / 38,944 | 389 -> 373 KiB |

The one-process cost rises 44-60 KiB as more layouts are shared; the per-realm cost falls. On the
later `c2907468` tree, after the shared-value changes, the shaped core `hold` was 1,356 / 19,092
KiB at 1 / 100 realms. With the 64-entry local key table it was 1,352 / 18,632 KiB in the same
session (five-run medians, with 100-realm readings spread across 18,360-18,660 KiB). The table
change saves about 4.6 KiB per added realm; the resulting slope is about **175 KiB per realm**.
Those numbers supersede neither the earlier 263-KiB audit nor its configuration: they are an
isolated comparison of this change, and the earlier audit used a different binary.

A live heap profile of 100 scheduled core realms on `c2907468` after the shape change, before the
table change, attributed per-realm retained allocation payload as follows: about 31 KiB in
`kiln_c99_reserve` property arrays (four large built-in stores account for about 25 KiB), 12 KiB
in realm-local shapes, 6 KiB in the local key table, 4.4 KiB in the C heap context, 2 KiB in
the module block, 2 KiB in provider callable stores, plus scheduler and output state. The
collector's `SIGUSR1` report for an idle realm was 46,021 bytes / 570 blocks, with 22 local
shapes and 16 local keys. The profiler also counted one 512-KiB nursery allocation and two
128-KiB slab allocations per realm; those are aligned *reservations*, not resident bytes, but
the pages used by the bootstrap remain resident. Allocation payload and resident memory are
different measures and should not be summed.

**The tens-of-KiB target remains open.** The largest built-in stores pass the
32-slot shared-shape limit, and the remaining 46-KiB collector heap, dynamic keys/shapes, and
allocator pages are real per-realm costs. Merely raising that fixed limit would enlarge *every*
shared shape and its 32-bit masks, including the many small ones. The next representation needs
a wide, process-shared descriptor layout for built-in stores, with realm-owned compact value
slots and copy-on-write behavior for user modifications; it must preserve own descriptors,
reflection, prototype changes, and the no-write shared read path. That deserves its own design
and implementation step rather than treating a pointer to another realm's ordinary object as
shared. A smaller initial key table and the shaped-definition path make the measured stage
reviewable without making that unsound shortcut.

### 2026-09-27 -- wide shape experiment and revised design (`work/memory-wide`)

The next experiment tested whether the remaining large built-in stores could simply stay in
shape mode longer. In an owned worktree at `dd287482`, a prototype changed `kiln_shape` to keep
32 keys/attributes inline and allocate a separate descriptor array for wider layouts; the
shape's attribute masks and slot limit were tried at 64, 128 and 256 entries. Values stayed in
each realm's own slots. A C fixture built a 224-member shared layout, defined an accessor above
slot 128, and checked the materialized descriptors after conversion to dictionary mode. It
passed. The prototype and fixture were removed after measurement; no production code from this
experiment was retained.

The same core `hold` source (`console.log("hi"); setTimeout(() => {}, 1500)`) was built at
`native-release` in the baseline and experiment worktrees, then measured under
`KILN_REALM_SCHEDULER=1`, `KILN_PARALLELISM=2`, and 1/2/16/100 realms. Values are five-run
medians of `/proc/<pid>/smaps_rollup` anonymous KiB at 0.55 seconds. The first line is a fresh
baseline binary from the same measurement session; the later lines include the process-wide
cost of each wider shape tree.

| maximum shaped members | 1 / 2 / 16 / 100 realms (KiB) | 1-to-100 slope (KiB/realm) |
| --- | --- | ---: |
| 32 (baseline) | 1,344 / 1,536 / 4,048 / 19,052 | 179 |
| 64 | 1,396 / 1,584 / 4,028 / 18,680 | 175 |
| 128 | 1,468 / 1,656 / 4,148 / 18,936 | 176 |
| 256 | 1,504 / 1,688 / 4,200 / 19,156 | 178 |

The 256-slot trial offered no useful resident-memory reduction and raised the one-realm base by
160 KiB. Its shared shape tree copies every prefix's keys and attributes, so its storage grows
quadratically with the number of members in a large store. A diagnostic build observed property
record reservations of 128 to 256 at member 129, and separate 0-to-128 and 0-to-64 reservations
at 103 and 33 members. Those are allocation events, not proof the resulting arrays were all live
at the sample. The idle collector report was essentially unchanged: baseline 47,989 bytes / 566
blocks / 17 local shapes; the 256-slot trial 47,893 bytes / 566 blocks / 17 local shapes. This
rules out raising the generic shape limit as the path to tens of KiB per realm.

**Next representation.** Give the bootstrap's built-in stores one process-shared *final*
descriptor table per stable layout, instead of a shape for every prefix. Each table holds the
ordered keys, attributes and method code; each realm holds only its values, prototype and object
flags. A realm's first redefinition or deletion gets a realm-owned overlay (with tombstones and
the original insertion order), or converts that one store to dictionary mode. Reads and
reflection consult the immutable table plus that realm's overlay; the no-write path only reads
published table memory and realm slots, and must neither intern nor materialize an overlay.
Inline caches can guard on the table identity and the realm object's mutation token. The table
should be emitted from the bootstrap's known install sites or frozen from one completed builder,
then published once; constructing it by running the generic shape transition chain repeats the
quadratic allocation above. Separately, even eliminating every remaining property array would
leave roughly 48 KiB of live collector objects per idle core realm plus allocator pages, so the
tens-of-KiB target also needs lazy built-in materialization and/or denser allocator pages. Each
part must keep built-in mutations and prototype links realm-local.

### 2026-09-28 -- compact dictionary records (`work/memory-layout`)

The dictionary fallback still owns a separate record for each built-in property in each realm.
Each record previously carried a second copy of its interned key's length and six full `int`
flags. The record now reads the length from the canonical key and packs the six Boolean flags;
the compiled C fixture reports **64 bytes per record, down from 88** on this 64-bit host. The
backend-C crate tests passed 225/225, and its emitted-C tests passed 36/36. One module-emission
snapshot was re-recorded using `INSTA_UPDATE=always` because the generated namespace code no
longer writes the removed field. The full native parity and stress gates remain due.

This is a bounded allocation saving, not a claim that the realm target has been reached. The
earlier profile attributed about 31 KiB of retained payload per realm to property arrays; at
identical capacities, a 24-byte record reduction would save about 8 KiB of that payload. A
same-commit resident-memory comparison could not run in the current sandbox: even
`kiln --one-shot build` failed before compilation with `cannot listen ... daemon.sock:
Operation not permitted (os error 1)`. No new RSS slope is claimed here.

The proposed final shared descriptor table is a separate representation change. The current
runtime accesses `object->properties` directly in 386 sites across 29 files, including own
lookup, reflection, enumeration, copy/spread, cache validation, GC marking, and the no-write
parallel path. Replacing the array with a shared key/attribute table and realm-owned values
requires those paths to use a common accessor and requires a mutation fallback before the first
table is published. Simply aliasing a first realm's records would expose its values, accessors
and mutable flags to other realms. The target remains open for that change and for the residual
collector and allocator pages described above.

The integrated native actor test caught two consumers outside the backend-C unit fixtures:
generated namespace cleanup called a helper private to another C translation unit, and the
standalone CLI error reporter still decoded the old 88-byte property layout. The namespace
path now uses the public string comparator, and the reporter mirrors the complete 64-byte
record stride. The native actor test again passes, including its extension-off error text.

### 2026-09-28 -- same-binary RSS after compact records

With the socketless `--one-shot` command path available, the core `hold` program was freshly built
from `f44e1e30` at `native-release` and measured from `/proc/<pid>/smaps_rollup` at 0.55 seconds
into its 1.5-second timer. Each run used `KILN_REALM_SCHEDULER=1`, `KILN_PARALLELISM=2`, a minimal
environment, and a private `KILN_REALMS_OUT`. Median anonymous KiB of three runs per count:

| realms | 1 | 2 | 16 | 100 |
| --- | ---: | ---: | ---: | ---: |
| anonymous KiB | 1,328 | 1,512 | 3,896 | 18,192 |

The 1-to-100 slope is about **170 KiB per added realm**. This is a current-state measurement,
not an isolated before/after causal estimate for the 64-byte record; it used a different binary
and measurement session than the 175-KiB result above. The target of tens of KiB remains open.

### 2026-09-28 -- smaller inline object slots

Each ordinary object carried sixteen inline shaped-value slots, even when its shape used fewer.
The slot count is now eight; a wider object uses its existing separate slot array. The parallel
worker's object-literal limit follows the same eight-slot boundary, so its allocator never tries
to define a ninth member without space. The generated literal fast path already stops at eight.

I built the core `hold` source from the same revision with 16, 8, and 4 slots, all at
`native-release`, then sampled `/proc/<pid>/smaps_rollup` anonymous memory 0.55 seconds into its
1.5-second timer. Each cell is the median of three runs of the *same binary* for that row and
realm count, with `KILN_REALM_SCHEDULER=1`, `KILN_PARALLELISM=2`, a minimal environment, and a
private output directory. The 4-slot variant was only an experiment; 8 is the committed choice.

| inline slots | 1 / 2 / 16 / 100 realms (KiB) | 1-to-100 slope (KiB/realm) |
| --- | --- | ---: |
| 16 | 1,332 / 1,504 / 3,836 / 17,756 | 166 |
| 8 | 1,320 / 1,488 / 3,720 / 17,036 | 159 |
| 4 | 1,320 / 1,484 / 3,672 / 16,724 | 156 |

Eight slots save about **7 KiB per added realm**. Four save only another 3 KiB while moving more
objects to separately allocated slots. Alternating three runs each of the `class-poly` native
benchmark gave a 3.5-ms warm median and the same checksum for 16 and 8 slots. Backend-C unit
tests passed (227 tests); selected `native-realms` probes (`region-builtins-per-realm`,
`classes-and-callables`, `object-literal-shapes`, `define-site-cache`) matched 4/4 at 1, 2, and
8 realms, and matched 4/4 at 1 and 2 realms with `KILN_NATIVE_GC_STRESS=1`.

This is a bounded saving, not the final descriptor representation. Sharing even one built-in
store's descriptor array would require every direct `object->properties` reader, writer and GC
tracer to consult shared immutable metadata alongside realm-owned values or a copy-on-write
overlay. Those accesses currently span read, reflection, enumeration, caches and collection;
aliasing another realm's ordinary records would leak its values and violate ownership. The
tens-of-KiB target remains open.

### 2026-09-28 -- compact data/accessor payloads

The 64-byte dictionary record still held a data value *and* two accessor values, although the
`accessor` flag makes those representations exclusive. The payload is now a union: one `kiln_value`
for data, or a getter/setter pair. A compiled C fixture asserts a **48-byte record** on this
64-bit host and traces an object containing both kinds through the collector. The generated
module namespace initializer now writes only the data arm, and the CLI's standalone error
reporter uses the matching 48-byte stride. The backend-C library tests passed (227 passed, one
ignored), and the emitted-C suite passed 36/36 with its namespace snapshot re-recorded by
`INSTA_UPDATE=always`.

At unchanged array capacities, the earlier profile's ~31 KiB of property-array payload would
fall by a quarter, about 7.8 KiB per realm. That is a payload estimate, **not** a measured RSS
change: allocator size classes and page reuse can change the resident result. The shared final
descriptor and remaining collector/allocator representation work still stand between this
incremental saving and the tens-of-KiB target.

### 2026-09-28 -- scheduled-realm output capture

An allocation-owner probe found that the scheduled `KILN_REALMS` launcher retained an 8,192-byte
`open_memstream` buffer plus its `FILE` allocation for every realm, even when the program printed
only `hi\n`. The launcher now keeps the `FILE *` host interface through `fopencookie` on Linux
and `funopen` on macOS, with a 256-byte stdio buffer owned by the session and a capture buffer
that starts at 64 bytes on its first write. It still opens output files only when realms finish.
The empty-output case allocates no capture buffer; a failed growth marks the realm failed and
cleanup frees any partial capture.

For the same core `hold` source at `native-release`, with `KILN_REALM_SCHEDULER=1`,
`KILN_PARALLELISM=2`, a minimal environment, and anonymous KiB from
`/proc/<pid>/smaps_rollup` at 0.55 seconds, three-run medians were:

| capture | 1 / 2 / 16 / 100 realms (KiB) | 1-to-100 slope |
| --- | --- | ---: |
| `open_memstream` | 1,316 / 1,480 / 3,632 / 16,496 | 153 KiB/realm |
| 256-byte buffered cookie | 1,308 / 1,460 / 3,504 / 15,724 | 146 KiB/realm |

The change saves about **7.7 KiB per added realm**. Each realm's `hi\n` and status matched at all
four counts. A four-realm program with no output produced four empty files and zero statuses. A
16-realm probe printing 1,000 lines per realm gave the same 8,890 bytes per file with each
capture; seven alternating runs had 8.9 ms medians for both launchers. This is a focused
throughput sanity check under load, not a quiet-machine performance baseline.

The remaining ~146 KiB slope is still well above the tens-of-KiB target. The next representation
is the shared final descriptor table described above. Its access seam must replace direct
`object->properties` reads in lookup, reflection, enumeration, spread, inline-cache validation
and GC with a view over shared keys/attributes plus realm-owned values; every writer must
materialize a realm-owned dictionary before changing a descriptor. A table must be keyed by a
stable built-in identity, never by a first realm's object pointer, and later realms must populate
their own value slots without building the entire prefix shape chain. Until those consumers are
ported, aliasing property records or a prototype across realms would violate isolation.

### 2026-09-28 -- descriptor reads keep ordinary shaped stores shaped

`Object.getOwnPropertyDescriptor` no longer converts an ordinary shaped object to dictionary
mode solely to answer a read. A single projection function supplies the same property record to
the descriptor reader and to an actual dictionary conversion. It reads shared keys and attributes
from the shape, and the value from that realm's slot; mixed dense-element/list stores still take
the established conversion path. The focused C fixture covers data and accessor descriptors,
attributes, an absent name, and the later materialized records. The stressed `descriptors.ts`
native realm probe matched at 1, 2 and 8 realms.

This is an access seam for the eventual shared final descriptor table, not a measurable cut to
bootstrap memory by itself. In alternating three-run `native-release` hold measurements with the
same scheduler settings above, median anonymous KiB were 1,308 / 15,736 for the prior binary at
1 / 100 realms, and 1,304 / 15,736 for this change. The slope remains about 146 KiB/realm.

### 2026-09-28 -- ordered-property read seam and final-layout staging

`Reflect.ownKeys` on an ordinary shaped object and own-key enumeration of a built-in's shaped
property store now walk the shape's shared keys without materializing a per-realm dictionary.
`kiln_c99_property_walk` gives an ordered count and `kiln_c99_property_view` projects a record
at an index. Mixed dense/list objects still use dictionary conversion; writers continue to
materialize before taking a mutable record pointer. The C fixture checks ordinary and built-in
key walks leave their stores shaped. This is the first reusable ordered-property read seam for a
final layout, and is a bounded memory saving for programs that reflect on shaped stores. The
minimal `hold` program does not perform that reflection, so its idle-realm slope should not be
credited to this change.

The final-layout identity for the first implementation should be the callable kind and stable
code already used by `kiln_method_store`: `(KILN_VALUE_BUILTIN, builtin.bits)` or
`(KILN_VALUE_METHOD, method.bits)`. Those bits identify the same built-in across realms, while
the `kiln_method_store` and its `properties` object are realm-owned. A region closure can join
later by an emitted region-site identity; ordinary function pointers, generated prototype
objects and a first realm's `kiln_object *` cannot key a shared table. Only shared canonical
keys may enter the table. A store with a realm-local computed key stays dictionary-backed.

Each process table should contain one *final* ordered key/attribute/code vector for a stable
store, without building every prefix shape. Each realm holds its own values (including accessor
pairs), prototype and flags. The first user redefinition, deletion or addition materializes that
one realm's dictionary from the table and values before modifying it. Reads, reflection, key
enumeration, spread, cache checks and GC must use the view or its corresponding value walk; an
inline cache may guard on table identity plus the object's mutation token. The table is
published once under the process lock after complete validation, with release/acquire readers.
The bootstrap must eventually fill later realms' value arrays directly; merely compacting its
finished dictionaries leaves the prefix allocation peak and allocator pages behind. The
remaining direct `object->properties` users and all mutation entries need an audit before any
object is allowed into this representation. Until then, no process-shared descriptor table is
installed, and issue 08 remains in progress.

### 2026-09-28 -- first active final built-in layout

The bootstrap's `Object.prototype` own-property store is the first active process-shared final
descriptor layout. A diagnostic pass over the core bootstrap found it ends with four list-mode
data properties and capacity four; `Date.prototype` ends with 47 and
`%TypedArray%.prototype` with 36, so neither fits the existing 32-bit shape masks. The stable
identity is `(KILN_VALUE_BUILTIN, KILN_C99_BUILTIN_OBJECT_PROTOTYPE)`, not a first realm's
object address. At bootstrap completion, a realm converts this one store's ordered records to
realm-owned inline values and attaches a process-shared, prefix-free shape holding only shared
keys and attributes. The first realm publishes the shape under the process lock; later realms
validate every key, attribute and numeric-slot bit before attaching. An incompatible layout
stays a dictionary. The shape does not grow transitions: adding or redefining a property
materializes that realm's dictionary; a plain value assignment writes only that realm's slot.
The ordinary shaped-object lookup, reflection, enumeration, cache guards and GC value tracing
serve this narrow representation without a new special case in their hot paths.

A focused C fixture checks two stores share one final layout while retaining different values,
descriptor and own-key reflection leave the layout attached, and addition and deletion detach
only the mutated store. The real `hold` bootstrap was observed attaching its four-property
`8:18` layout. Three stressed native realm probes matched at 1, 2 and 8 realms. With alternating
three-run `native-release` hold measurements under the same 0.55-second anonymous-KiB method,
the old and final-layout binaries both measured 1,304 KiB at one realm; at 100 realms they
measured 15,696 and 15,676 KiB. The ~20-KiB difference over 99 added realms is near the noise
of this machine and far below the target's missing ~100 KiB/realm. This first activation proves
the ownership/publishing path, not the wide-store economy. The bootstrap still builds and frees
the four-record dictionary, so its allocation peak remains. The later experiments below bound
the likely benefit of extending this representation. Issue 08 remains open.

### 2026-09-28 -- residual memory experiments and architecture decision

The `Date.prototype` wide-record experiment is preserved on branch `work/memory-region` but is
not part of this issue's implementation. It shared 47 sealed region-closure records and saved
about 3.0 KiB per added idle realm at 100 scheduled realms. An ordinary Date call materialized
the private dictionary on first read, reducing the saving to about 0.8 KiB per realm. Its
Date-specific marker would add a read-path representation without a material benefit in active
programs. A generic view with copy-on-write mutation is possible, but the two wide built-in
stores (Date and `%TypedArray%`) together hold only about 6 KiB of dictionary capacity per realm;
including the global and process stores is unlikely to remove more than another few KiB.

Two allocator experiments did not provide a larger cut. Widening small-object slab size classes
from 16 to 32 or 64 bytes left the 100-realm anonymous-RSS slope within measurement noise. After
a forced post-bootstrap collection, 391 live nursery blocks occupied 14 of the 16 pages touched
by bootstrap; decommitting only dead pages can reclaim at most about 8 KiB, while the forced
collection itself previously increased RSS. Skipping the entire eagerly built
`process.__kilnNativeModules` tree saved only about 0.8 KiB per realm and would change observable
behavior. All three diagnostic changes were discarded.

The measured scheduled core-realm slope remains about 146 KiB per added realm, versus the
tens-of-KiB target. Reaching that target needs a change to how the live bootstrap objects and
their allocator pages are represented or created, not another isolated descriptor or slab tweak.
The current collector and runtime keep stable object addresses, so a moving compaction design
would first need a separate audit of generated pointers, host handles and GC roots. Issue 08
remains open pending that design and the final native-stress gate.

### 2026-09-28 -- allocator bound and retained-heap profile

The next allocator hypothesis was checked on the same `native-release` `hold` binary at
`KILN_REALMS=1` and `=100`, sampled during the 1.5-second timer. Anonymous resident memory
was 1,292 and 15,704-15,712 KiB with glibc, about 145.6 KiB per added realm. Replacing the
allocator via `LD_PRELOAD` with jemalloc gave 2,108-2,112 and 15,752-15,788 KiB, about
137.9 KiB per added realm; its larger fixed cost makes a single realm worse. Mimalloc was much
worse (13,044 and 62,716-87,900 KiB), while minimal tcmalloc (5,464 and 19,712-21,720 KiB)
did not improve the slope reliably. These are same-binary trials, so compiler output and
bootstrap code are held fixed. An allocator swap can reclaim roughly 8 KiB per realm here, far
short of the roughly 100 KiB reduction needed for the tens-of-KiB target.

An `LD_PRELOAD` malloc interposer, differencing live allocations at one and 100 realms at
0.55 seconds (after all 100 realms had bootstrapped), found
about 48.9 KiB of added *small malloc allocation payload* per realm (call sites whose average
allocation is under 8 KiB). The largest sources were four property arrays (13,824 B), about
25 realm-local shapes (10,429 B), the C heap context (4,496 B), and dictionary conversions
(3,456 B). The nursery and slab backing allocations are separate: alignment makes malloc
request about 512 KiB for a 256 KiB nursery and about 128 KiB for each of two 64 KiB slabs.
Those request sizes are **not** resident costs and must not be added to the RSS slope. The
interposer's small-allocation figure also excludes malloc metadata and page rounding.

A temporary diagnostic forced a collection just after bootstrap and counted each remaining GC
block; the diagnostic was removed after measurement. There were 405 live blocks with 26,602 B
of payload in 42,352 B of rounded slots. The kind breakdown was:

| kind | blocks | payload | rounded slots |
| --- | ---: | ---: | ---: |
| ordinary object | 96 | 19,200 B | 23,040 B |
| cell | 224 | 1,792 B | 10,752 B |
| array | 19 | 3,280 B | 3,888 B |
| method store | 33 | 1,056 B | 2,112 B |
| string | 28 | 842 B | 1,968 B |
| collection and symbol | 5 | 432 B | 592 B |

The nursery allocation spans 16 four-KiB pages after that collection, and the two slabs have
touched about one page each: roughly 72 KiB of GC-backed pages versus 41.4 KiB of live slots.
This is a page-occupancy estimate after a diagnostic collection, not a byte-for-byte attribution
of the unmodified binary's RSS. `/proc/<pid>/smaps` on the unmodified binary measured 1,296 KiB
anonymous at one realm and 15,676 KiB at 100 realms (145.3 KiB/added realm); almost all added
memory was anonymous mappings, not executable or shared-library pages. The 48.9 KiB small
malloc payload plus roughly 72 KiB GC-backed pages leave about 25 KiB/realm for allocator
rounding, thread/runtime state, and other anonymous mappings; the two instruments cannot split
that remainder reliably.

This profile does not support a narrow property-array allocator as the next implementation.
Even eliminating *all* 13.8 KiB of property-array payload would leave the slope above 130 KiB,
and those arrays hold stable per-object pointers that must survive free/reallocation. The GC
footprint is likewise distributed across ordinary objects and cells, not one optional bootstrap
subtree: removing the whole native-module tree earlier saved only 0.8 KiB. A substantial cut
needs a general design that reduces the count or representation of realm-owned bootstrap
objects, their cells and their mutable property values while retaining per-realm identity,
reflection and copy-on-write semantics. No allocator or GC implementation from this profile
was retained; issue 08 remains open.

### 2026-09-28 -- compact once-made cells, and the remaining layout

The first general compact realm layout moves *once-made bootstrap cell payloads* into the
realm's existing unit block. The `module_cells` proof already finds a cell created once in a
top-level or called-once body, on no cycle, and rewrites every closure capture of it to read
through that realm's unit block. The emitter now appends one eight-byte cell value per such
binding to the block and makes the local/global cell tag point at it, instead of allocating a
48-byte GC slab slot for an eight-byte value. It preserves the existing tagged-cell read,
write, dead-zone and closure paths. The block is allocated once and never moved; a realm's
collector marks every appended value as a root and its graph-transfer incoming-root check
also visits them. A module with imports or exports keeps GC cells because a cell pointer can
cross a generated-unit boundary. There is no compact cell in the process-shared region:
values and pointers are realm-owned, and every realm has its own unit block.

The ownership audit is narrow. The cell tag is handled before any GC block-header read when a
`kiln_val` crosses to a boundary value. A compact cell pointer does not identify a GC block,
so the collector cannot discover its contents by tracing the pointer; tracing the appended
unit slot is required. The unit block is freed only during realm teardown, after its frames
have finished. Once-made cells cannot be captured in a non-rewritten closure, or this layout
would let a pointer outlive its unit's proof. The `module_cells` pass remains the only gate that
can select one. Ordinary cells, per-call closures, imports and exports stay GC-owned.

On three alternating, same-binary `native-release` `hold` comparisons with valid per-realm
output directories, the median anonymous KiB at one/100 scheduled realms was 1,292/15,708
before and 1,284/14,788 after. Every one of the 100 realms printed `hi` and exited zero in
every run. The added-realm slope fell from 145.6 to 136.4 KiB, about 9.2 KiB per realm. This
matches the previous profile's 224 live cells using 10,752 B of rounded GC slots, offset by
the compact unit block's added values. It is a worthwhile first slice, not the target.

Focused checks: `mbx test -p kiln-backend-c` passed 231 unit tests; the emitted-C ABI snapshot
was recorded with `INSTA_UPDATE=always` and its focused test passed. The kiln-native emission
test passed for compact cells and the exported-binding guard. Under collector stress,
`scripts/native-realms.sh` matched `region-builtins-per-realm`, `dates` and
`classes-and-callables` at 1, 2 and 8 realms. Direct interpreter/native comparisons with the
same stress setting matched those three probes, including output and exit status. The final
program-wide `mise run native-stress` remains due.

The next layout has to treat the 96 live ordinary objects, their property arrays and realm-local
shapes together. Their observed live payload/slots alone are about 19.2/23.0 KiB of objects,
13.8 KiB of four property arrays and 10.4 KiB of shapes per realm; trimming any one of those
leaves most of the slope. A process-wide sealed descriptor graph could hold stable keys,
attributes and method codes once, while each realm keeps dense mutable values and thin
identity handles. Reads and reflection would use a descriptor/value view without creating a
dictionary; redefinition, deletion or extension would materialize only that realm's store.
To make that an actual tens-of-KiB cut, the bootstrap must *construct directly into* the
descriptor/value layout or materialize objects lazily: compacting its finished dictionaries
does not reclaim their allocation peak. This requires auditing every direct object-property,
inline-cache, prototype and collector path before activating a wide store. Per-realm object
identity, own-key order, descriptors, prototype mutation and GC reachability are required
parity gates. The current Object.prototype final layout is the narrow validation seam; its
four properties do not establish the wide layout's memory economy. Issue 08 remains open.

### 2026-09-28 -- wide bootstrap-object layout bound and activation audit

A temporary post-bootstrap live-object walk on the compact-cell baseline counted 96 ordinary
objects (76 shaped, 20 list). The list property arrays together held 17,472 B of capacity.
The largest were a 95/128 store, inferred to be the global object (6,144 B), `Date.prototype`'s 47/64
(3,072 B), `%TypedArray%.prototype`'s 36/64 (3,072 B), and two more stores at 24/32 and
15/16 (1,536 and 768 B). The diagnostic was removed. Even an ideal shared descriptor plus
eight-byte realm-value view for **every** list store can save only about 15 KiB of property
payload, before its own metadata and fallback costs. Date and typed-array stores alone are
bounded near 5.5 KiB. The slope is about 136 KiB, so activating another wide-store marker
without changing object construction or identity is not a material path to tens of KiB.

The concrete next design and access audit are in
[bootstrap-object-layout.md](../bootstrap-object-layout.md). It uses compiler-proven once-only
bootstrap object sites, a sealed process descriptor, stable realm-owned identity handles and
dense realm value slots, with copy-on-write dictionary materialization. Later realms must
construct directly into the slots to avoid the dictionary allocation peak. This requires a
general read/reflection/IC/GC view and a borrowed-result teardown proof before activation;
there are 370 direct `->properties` uses across 27 runtime C files to classify, along with
77 `ensure_dictionary` and 190 `object_of` references. No wide-layout code was retained from
this audit. Issue 08 remains open.

### 2026-09-28 -- descriptor write boundary for bootstrap globals and methods

Property storage now accepts a complete own data descriptor (key, realm value and final
attributes) in one call. The global bootstrap uses it for its built-in names, realm-owned
objects, locked constants and self links, so it no longer creates records and later sweeps
through writable pointers to hide them. `defineMethod` uses a property-storage commit boundary
for its list-mode fallback; it still preserves assignment semantics, including inherited
setters and read-only properties, before making the resulting method non-enumerable. The
ordinary assignment and shaped-method paths keep their existing behavior. This is an access
seam for a later descriptor/value store, not an active shared wide layout: bootstrap still
allocates realm dictionaries, and the roughly 136 KiB added-realm memory slope is not claimed
to have reached the target.

Focused native comparisons matched 8 selected probes, including a new global descriptor,
mutation and key-order stability probe; collector stress matched the original 7 probes and
the new probe separately. Realm parity matched the new global probe and a method-cache probe
at 1, 2 and 8 realms; the new global probe also matched at those counts with collector stress.
The native `Reflect.ownKeys(globalThis)` sequence matched the unchanged base binary byte for
byte (1,048 output bytes), accounting for the engines' pre-existing global insertion-order
difference without changing it.
The backend-C crate had 233 passing tests and one synthetic graph-root
fixture failure that reproduced on the unchanged base commit; that fixture is fixed separately.
In a same-source 1,000-realm scheduled startup comparison, median wall time was 0.262 s before
and 0.267 s after (three alternating runs); the difference is within the observed run-to-run
spread. Issue 08 remains open.

### 2026-09-28 -- direct global construction activation audit

The descriptor-write API is necessary but insufficient for a live process-sealed global.
Compiled built-ins delete many temporary `globalThis.__kiln*` hooks immediately after the C
global is constructed. A sealed initial table with a copy-on-write dictionary at first deletion
would materialize on every normal bootstrap and lose its per-realm allocation saving. Sealing
only after those deletions still requires the earlier bootstrap to build a dictionary unless
reads, deletions and additions operate on a realm-owned overlay throughout bootstrap.

The exact read/reflection/mutation/IC/GC sites and the required projection contract are in
[bootstrap-object-layout.md](../bootstrap-object-layout.md#global-direct-construction-activation-blocker-2026-09-28).
`global_binding` itself reads `object->properties` directly, so activation before that refactor
would lose bare global names. The current `kiln_shape` is limited to 32 properties and cannot
stand in for the 95/128 global list. That list's 6,144 B allocation capacity is also too small
an upper bound to justify a global-specific virtual marker on its own. No active wide-layout
code was retained, no memory improvement is claimed, and issue 08 remains open.

### 2026-09-28 -- read-only property projection, first access slice

`kiln_c99_property_read_own` now returns a copied read-only property record from a shape slot,
dense element or dictionary list without converting shape storage. Ordinary member reads,
bare global bindings and `getOwnPropertyDescriptor` use that boundary, preserving accessor
calls, refused members, attributes and realm-owned values. The shape IC fast path still reads
its guarded slot directly; its generic fallback reaches the projection. A storage-owned,
allocation-free GC edge function now traces the same existing shapes, elements and list records,
so a later overlay has one trace entry point rather than changing the collector's object-kind
switch. No descriptor/value overlay is active. Own-key enumeration, spread, mutation and graph
incoming-root proof still need their direct-list assumptions removed before one can be enabled.

Focused backend-C tests passed 235 unit cases and 36 integration cases after the prelude ABI
snapshot was re-recorded with `INSTA_UPDATE=always` and the measured prelude-size ceiling was
updated (61,502 lines). Twelve selected native probes matched the interpreter both normally
and under collector stress. Three descriptor/accessor probes matched at 1, 2 and 8 realms;
`parallel-heap-reads` matched at its 1, 2 and max parallelism settings, exercising the no-write
read path. `symbol-own-keys` still diverges in one descriptor line, but its native output was
byte-identical to the unchanged pre-projection binary; that parity issue is tracked separately.
A native-release property-read workload returned the same value on both binaries; seven
alternating runs had 44.8 ms baseline and 44.6 ms changed median. This slice does not claim a
memory reduction or resolve issue 08.

### 2026-09-28 -- projected own-key, spread and JSON walks

`Object.keys` now projects each ordinary named property through `kiln_c99_property_view`;
`for...in` still has its separate shadowing walk. Generic object spread and JSON iteration use
`kiln_c99_property_walk` and copy each projected record before invoking a getter, so a getter
cannot invalidate a borrowed record. The existing mixed dense/list conversion remains, and
mutations still obtain a writable dictionary record. The wide descriptor/value overlay is not
active, so this slice claims no RSS reduction.

Backend-C tests passed 236 unit and 36 integration cases with an `INSTA_UPDATE=always` ABI
snapshot. Eleven selected native probes matched, and a new dense-key/getter-append probe matched
normally and under collector stress. A stronger probe where a spread getter deletes an earlier
key exposed an older live-index behavior: native copies a newly appended key instead of the
original later key. The previous direct dictionary loop indexed the same live array, so this
predates the projection. A future general cursor must snapshot keys to fix this and support a
wide sealed descriptor view. Issue 08 remains in progress.

### 2026-09-28 -- stable spread key cursor

The generic ordinary-object spread now snapshots its original own keys in a GC-traced array,
then looks up each current descriptor by key through `kiln_c99_property_read_own`. A getter that
deletes an earlier property can no longer shift a later key out of the spread or substitute a
newly appended key. A deleted later property is skipped, while a changed value or enumerable
attribute is read at its turn. The earlier direct-index walk had diverged from Node and the
interpreter on that mutation. This is the key-cursor contract needed by a future wide
descriptor/value overlay; the overlay is still inactive and issue 08 remains open.
Backend-C tests passed 236 unit and 36 integration cases. Three selected native probes,
including the new mutation case, matched both normally and under collector stress.

### 2026-09-28 -- property-storage mutation boundary

The ordinary slow assignment path, own descriptor commit, deletion and freeze now enter
storage-owned operations in `properties.c`. `defineProperty` validates and changes a copied
descriptor, committing it once after every field and invariant check; it no longer carries a
writable dictionary pointer through descriptor semantics. Fresh assignment and bootstrap data
definitions use the same commit. Delete and freeze retain today's ordered-list behavior inside
the storage boundary. `Object.seal` is implemented by the TypeScript bootstrap through
`defineProperty` and `preventExtensions`, so it reaches this boundary through descriptor commit.
The fast shaped slot assignment remains guarded by a shape and still writes that representation
directly. This prepares the wide overlay's mutation seam but does not activate or measure it.
Focused backend-C tests passed 237 unit and 36 integration cases with a regenerated ABI
snapshot. Eight existing native probes and a new seal/freeze integration probe matched; three
mutation probes matched under collector stress. `symbol-keyed-inline-caches` had a byte-level
divergence with visually identical output on both this branch and its unmodified baseline.
A same-source native-release loop of 100,000 assignment/redefinition/delete/freeze cycles
returned `300000` on both binaries; nine alternating runs measured 106.67 ms baseline and
107.80 ms changed median (about 1.1% slower). Issue 08 remains in progress.

The focused `kiln-native` test `native_node_fs_promises_read_write_stat_and_encoding` fails
identically on the pre-mutation-boundary commit `36aa1cdf` and the integrated mutation-boundary
commit `02c9947a`. Both produce `héllo,true,6,6,true,false,true,true,undefined,undefined,false`
through the final promise rejection handler. The rejection is `TypeError Cannot delete property
'@@kiln:node:is-file' of an object` at the fixture's strict `delete` after its
`Object.defineProperty` call. This is a pre-existing reserved-key behavior question, not evidence
of a regression from the storage boundary; it needs separate triage before a full native crate
run can pass.

### 2026-09-28 -- projected reflection and stable descriptor keys

`Object.defineProperties` and the ordinary `Object.values`/`Object.entries` paths now snapshot
own keys before invoking a descriptor getter and re-read each descriptor through the storage
projection at its turn. `Object.keys` and both `for...in` dictionary walks use projected records
instead of indexing mutable property lists directly. This keeps a getter's deletion or
addition from shifting the iteration cursor and leaves later descriptors subject to their current
enumerability and value. Spread uses the same key-snapshot helper. The sealed wide layout remains
inactive: this is a reader prerequisite, not an RSS reduction. Selected native parity matched six
probes, including the new reflection probe; three probes matched with collector stress. Issue 08
remains open.

### 2026-09-28 -- closed-world native bootstrap family selection

Native builds now use retained member/global demand to omit TypeScript bootstrap installers for
intrinsic families a closed-world program cannot reach. Computed access, `globalThis`, dynamic
evaluation/import, Node mode and full method-group demand retain the complete bootstrap. The
selector closes over dependencies (for example typed arrays, Atomics and structured clone retain
the ArrayBuffer installer), retains literal/iterator-reachable installers unconditionally, and
deletes private host hooks belonging to omitted installers. Each selected source keeps the
original installer order and semantics for retained families. A 16-entry bounded compilation
cache holds source variants without leaking every possible 10-bit family mask in a build daemon.

In the same-source `native-release` timer hold probe, with a minimal environment and scheduled
1/100 realms at the 0.55-second sample, three alternating runs measured median anonymous RSS of
1,276/14,676 KiB with the full bootstrap and 936/11,464 KiB with selected families. The
added-realm slope fell from 135.35 to 106.34 KiB, about **29.0 KiB per realm**. Output and exit
status matched. Nine focused native probes covering Date, typed arrays, ArrayBuffer, Atomics,
structured clone, Promise, WeakRef, iterator helpers and parallel methods matched the
interpreter. The selector, source cleanup and emission-cache unit tests passed. Four selected
Date/typed-array/ArrayBuffer/parallel probes also matched with collector stress.
Date, typed-array and parallel probes matched at 1, 2 and 8 simultaneous realms.

This cut helps closed-world native programs; dynamic and Node programs still construct the full
realm bootstrap. A deliberately incomplete source-strip measurement, used only to bound the
opportunity and never retained, left roughly 96 KiB per added realm. Thus further source
pruning alone cannot reach the tens-of-KiB target. The next material mechanism must represent
the remaining bootstrap graph with thin realm identity handles, process-sealed descriptors and
realm-owned value slots, and construct directly into that form after the first realm. The
roughly 23 KiB of ordinary-object slots, 14 KiB of property-array payload and 10 KiB of local
shapes are a joint target; removing one alone is insufficient. The retained 14 nursery pages
and host/runtime overhead must then be remeasured. Issue 08 remains open.

### 2026-09-28 -- complete process descriptor publication, no realm overlay yet

The process now publishes immutable, complete key/attribute vectors for stable Date.prototype,
%TypedArray%.prototype and global stores after the first realm's bootstrap. Publication allocates
and validates the entire data-property sequence privately, requires every key to be canonical and
process-shared, then inserts the whole node under the process lock with a release store. An
acquire lookup never sees an unfinished prefix. The table holds no property value, prototype or
realm object pointer. A temporary diagnostic, removed before commit, observed 47 Date entries,
36 typed-array entries and 93–94 global entries depending on native bootstrap selection. The
different global counts mean a later realm must validate the complete layout and fall back if it
does not match; keying only by the global site is insufficient.

No realm uses this table for storage yet, so this slice claims no RSS reduction. Generated C99
validation passed, three selected native probes matched, and Date/typed-array probes matched at
1, 2 and 8 realms. Next is a realm-owned slot/visibility overlay and complete projected
read/mutation/GC paths, then direct construction on the second realm. The global additionally
needs insertion-order handling for temporary `__kiln*` host hooks that are deleted during
bootstrap; a prefix-only visibility bitmap would change observable key order. Issue 08 remains
open.

The next prerequisite adds a dormant realm-owned sealed-store field to ordinary objects, with
copied property projection in descriptor/insertion order, GC tracing of dense realm values and
graph incoming-edge inspection of those slots. All allocation paths initialize the field to
NULL; no object attaches a sealed layout yet. This lets readers and PIC guards move to the
projection before the first direct-construction activation, with no RSS claim for this seam.

### 2026-09-28 -- active sealed stores after bootstrap

Date.prototype, %TypedArray%.prototype and the global now adopt a process-sealed descriptor
after their bootstrap completes, but only if every ordered key and final attribute matches the
published table. Their values remain realm-owned; reads and reflection project those values
without building a dictionary. A mutation that needs different storage materializes the complete
ordered dictionary in that realm. `KILN_DISABLE_SEALED_LAYOUTS=1` retains the old storage for a
same-binary measurement and diagnosis.

With the same `native-release` hello-plus-timer binary, minimal environment, 0.55-second sample
and three runs of 1 and 100 scheduled realms, the disabled path had median anonymous RSS
1,216/11,728 KiB; the active path had 1,208/11,264 KiB. The added-realm slope fell from 106.18
to 101.58 KiB, saving about **4.6 KiB per realm**. All 100 realm outputs and exit statuses
matched. Generated C99 validation and thirteen targeted native parity probes passed. This is a
real but small steady-state cut: bootstrap still allocates temporary dictionaries, and the slope
remains far above the tens-of-KiB target. Direct construction and retained-page accounting are
next; issue 08 remains open.

Three Date, typed-array and global probes also matched under native GC stress and at 1, 2 and 8
concurrent realms.

### 2026-09-28 -- direct construction into sealed callable stores

Once the first realm publishes a Date.prototype or %TypedArray%.prototype descriptor, later
realms create that callable store with its process descriptor and empty realm-owned values.
Bootstrap's complete data-property and method writes fill those values and insertion ordinals
directly. A key or attribute outside the published descriptor, a write requiring an inherited
setter, or a later reconfiguration falls back to the ordinary dictionary. The direct path is
limited to these two callable stores; the global still builds its temporary host-hook keys and
adopts a sealed descriptor after bootstrap. `KILN_DISABLE_SEALED_DIRECT=1` retains the
post-bootstrap-only path for same-binary attribution.

In a `native-release` Date.getTime-plus-timer hold, using the same binary, minimal environment,
three runs and the 0.55-second sample at 1 and 100 scheduled realms, median anonymous RSS was
1,364/12,456 KiB with all sealed storage disabled, 1,364/11,808 KiB with post-bootstrap adoption
only, and 1,348/11,712 KiB with direct construction. Direct construction saved about **0.8 KiB per
added realm** beyond post-bootstrap adoption. All 18 runs had identical output and zero statuses.
The direct writer is a correctness foundation, but the remaining slope is still about 105 KiB;
expanding it to other individual stores without a larger page or object-layout mechanism is
unlikely to reach the tens-of-KiB objective. Issue 08 remains open.
Generated C99 validation and thirteen targeted native parity probes passed; Date, typed-array
and global probes matched under collector stress and at 1, 2 and 8 concurrent realms.

### 2026-09-28 -- retained allocation and large-realm RSS after direct construction

The direct-construction tree (`8600f266`, including the emitted-C snapshot) was profiled with a
single stripped `native-release` Date.getTime-plus-30-second-timer binary, a minimal environment,
and the scheduled-realm launcher. At 8 seconds, 1,000 realms used 111,048 KiB from `smaps`:
99,972 KiB anonymous, 3,116 KiB `[heap]`, and 7,904 KiB file-backed. Two 10,000-realm samples
used 1,027,344 and 1,027,960 KiB, of which roughly 976,000 KiB was anonymous and 43,500 KiB
was `[heap]`; a third complete run plateaued near 902,000 KiB. All 1,000 and 10,000 realms in
the complete runs printed the same `0` and exited with status zero. The large-count slope is
therefore variable, about **88–102 KiB per added realm**, but remains linear rather than a fixed
small-count allocator cost. The binary SHA-256 was
`465d4d3f09e02959ba0c7bbd080625fd294d6c9692265a817637e827c9c4dc17` and the input
SHA-256 was `fa5a93c721cce4299ba615d2a0d11f3b43c970f6cff43d746bff94e329721922`.

A temporary malloc interposer sampled the same source at 1 and 100 realms. A symbol-retaining
release relink (the only diagnostic build difference) attributed about **34.0 KiB per added realm**
to small live malloc requests. The largest were 11.4 KiB in roughly 28 realm-local `kiln_shape`
nodes, 4.6 KiB in the GC heap context, 2.1 KiB in the provider callable-store table, 2.0 KiB in
the realm context, and 1.8 KiB in compact unit-value backing. The nursery's 512 KiB request and
two roughly 128 KiB slab alignment requests per realm reserve address space; their requested
sizes must not be mistaken for resident pages. The prior retained-GC inventory found about
42 KiB of rounded live object/cell slots, which together with small malloc payload and allocator
page rounding explains why the direct two-store writer cannot reach the tens-of-KiB goal.

Further one-off sealed callable stores have a small bound. A material next cut must compact or
avoid ordinary bootstrap objects and cells across the remaining graph, and eliminate their
property/shape allocations as a group; it must preserve realm-local identity, values, mutation,
reflection and GC tracing. After that, remeasure touched nursery/slab pages and host allocator
residency at both 100 and 10,000 realms. No diagnostic build change was retained and issue 08
remains open.

### 2026-09-28 -- register fixed bootstrap keys before realm creation

An instrumented one-realm build identified all 24 retained local shapes in the minimal-env Date
probe. Four belong to the legitimately realm-local `process.env` chain. Five belonged to the
stream chain rooted at private `@@node:readable`, fourteen to Atomics' fixed C method-name
table, and one to bootstrap's ordinary-string `@@dispose`. The prelude's load-time literal
registration had inserted `@@` spellings only in the ordinary domain, while C's fixed-name
adapter uses a distinct private or symbol domain. The Atomics table was function-local and the
literal-key extractor never saw its entries. Once any first key was local, every child shape in
that object's chain was local even when the child's key was shared.

The registration now inserts both ordinary and fixed domains for static `@@` spellings and
registers Atomics' fixed names before the first heap. The ordinary and reserved domains retain
distinct canonical keys; dynamically created `process.env` keys remain realm-local. The
instrumented local-shape count fell from 24 to 4, and the diagnostic was removed. With identical
stripped `native-release` Date-plus-timer binaries built from the same source, minimal
environment and three alternating 1,000-realm samples at 2 seconds, median anonymous RSS fell
from 103,428 to 95,052 KiB, or about **8.4 KiB per realm**. Four alternating 1/100-realm
samples measured median added-realm slopes of 105.13 before and 96.77 KiB after, the same
8.4 KiB saving. The emitted-C suite passed 36/36 with its snapshot regenerated; seven native
probes matched, two matched under collector stress, and Atomics, stream, symbol and a new
string/symbol/private domain probe each matched at 1, 2 and 8 realms. The remaining slope is
still near 97 KiB, so issue 08 remains open.

### 2026-09-28 -- forced bootstrap collection and nursery-page reclaim rejected

On the fixed-key tree (`ca4d324a`), a temporary diagnostic forced a collection at bootstrap
completion and enumerated 144 retained GC blocks with 18,955 bytes of payload and about 23.2 KiB
of rounded slots. Seventy-five ordinary objects account for 18,000 bytes of those slots. The
survivors occupied at least eight nursery pages and one slab page; that diagnostic counted block
start pages, so a block spanning a page boundary can raise the actual live-page count.

A measurement-only experiment then forced that collection and used `madvise(MADV_DONTNEED)` on
whole pages in promoted nursery chunks only when no live block overlapped the page. Four
alternating 1/100-realm samples of the **same** stripped `native-release` Date-plus-30-second-
timer binary, under a minimal environment, measured median anonymous slopes of **96.87 KiB**
without the experiment and **114.89 KiB** with it. Promotion, a fresh nursery chunk and GC
bookkeeping outweighed the pages returned; this path is not a memory reduction. A separate
same-binary 64 KiB nursery setting gave about 93.05 KiB per realm versus 97.33 KiB at the
256 KiB default, a roughly 4 KiB ceiling on this probe before accounting for collection
frequency in allocation-heavy programs. Neither change was retained.

The remaining roughly 97 KiB slope cannot be explained by one dead-page leak. Post-collection
GC slots alone are roughly 24 KiB and the prior malloc profile attributes about 34 KiB per realm
to small live host allocations. Further work needs to eliminate groups of retained realm-owned
objects/cells and their backing allocations while keeping identity, value isolation, reflection
and tracing intact. Issue 08 remains open; do not treat a forced collection or smaller nursery
as its completion.

### 2026-09-28 -- retained callable-store group and representation limit

A temporary diagnostic on `cdb9ee4d` logged the callable-store allocations in a one-realm
minimal-environment Date-plus-timer run. Bootstrap creates **29 distinct built-in own-property
stores**, including Object, Array, Date, TypedArray, Map, Set, Error constructors and their
prototype identities. The realm registry retains these stores, so they are a coherent group for
one future representation. The diagnostic was removed.

Each store currently has a `kiln_method_store` routing record and a full ordinary `kiln_object`
identity. The prior retained inventory counted 75 ordinary objects after a forced collection;
all of their rounded GC slots together were 18,000 bytes. Thus removing *every* ordinary object
block is bounded at about 18 KiB of live slots before allocator-page effects; replacing only the
29 callable stores cannot by itself reach the tens-of-KiB target from the current ~97 KiB slope.
A temporary four-inline-slot build confirmed that merely shrinking each object is ineffective:
its alternating 1/100-realm samples were about 97 KiB per added realm, versus about 96 KiB with
eight slots, because objects crossing the inline limit allocate overflow backing. The smaller
layout was reverted.

A useful next activation must treat the 29 callable stores **and** the remaining bootstrap
objects/cells as one graph: allocate realm-stable thin identities for proven bootstrap sites,
keep ordered process descriptors free of realm pointers, and hold only values and mutations in
realm-owned dense backing. The existing sealed Date/TypedArray/global projection and mutation
boundary cover part of the read/write contract. The blocking audit is the rest of the direct
`kiln_object *` consumers: every property, prototype, GC, graph-transfer, host and inline-cache
path must accept a thin identity without reading full-object fields; a borrowed result must keep
its realm backing after teardown. Compiler-assigned stable site identities are needed for the
non-callable objects, whose allocation sites and eager bootstrap writes do not yet have the
once-per-realm proof. Until that joint boundary exists, replacing the 29 stores alone adds a new
object variant for a small upper bound. Issue 08 remains open.

### 2026-09-28 -- dense built-in registry control rejected

A temporary provider change routed the 29 built-in callable stores to a dense realm-owned
vector instead of the general hash map. Its focused identity and provider-root tests passed.
A first same-binary comparison appeared to save 12 KiB per realm, but the disabled branch was
selected by an added `KILN_DISABLE_DENSE_BUILTIN_STORES` environment variable. That variable is
visible through each realm's `process.env` and changed the workload being measured. An unchanged
`97228a93` binary, built in a separate owned worktree from the same Date-plus-timer input and
interleaved with the dense binary under identical minimal environments, measured 96.89 versus
96.83 KiB per added realm. The difference is noise; the dense implementation was reverted.
Future same-binary switches must keep the process environment identical on both sides, such as
using the same key with values `0` and `1`, before attributing an RSS change to the code path.
