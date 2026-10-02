# Candidate-scaled transfer admission

Status: implementation and shipping verification complete; see issue 10 and the durable shipping record.

Production certification inspects the candidate, its weak outside-owner witnesses and the
independent registered roots. It does not walk unrelated GC blocks, local shape indexes or the
canonical key table. `KILN_GRAPH_LEDGER=0` retains the previous sender-wide implementation;
`KILN_GRAPH_LEDGER_AUDIT=1` compares against that independent walk and refuses disagreements.
No fallback silently converts a failed proof into a copy.

## Authorities

| storage | admission authority |
| --- | --- |
| GC value and borrowed pointer fields | owner-carrying store helpers; weak owner witnesses; current typed owner layout at certification |
| Property descriptors and growth buffers | descriptor/value copy and relocation helpers; owner identity survives side-buffer relocation |
| Generated cache hits and construction | compact monomorphic/polymorphic write helpers; template and namespace publication helpers; generated-source guards |
| Callable storage aliases | prototype/store accessor guard, including copied and indexed aliases; explicit owner supplied to the store |
| CommonJS opaque contexts | independent weak inventory from construction, including contexts predating the factory; exact live visitor/anchor registration checked at admission |
| Module cells and provider state | one root visitor used by collection and read-only inspection; independent provider pass enumerates it once |
| Canonical keys | copied into realm-owned tenured allocations outside candidate capture, including large strings; insertion only accepts that constructor's output or process-owned shared keys |
| Native shapes and edges | native allocator ownership; a new shape edge validates canonical key identity; inherited keys retain that authority |
| Shared facade identity links | complete incoming layout including the hash chain; all facade-owned pointer writes carry their owner |
| Ordinary realm roots, queued work, weak resource handles, stack and registers | independent existing turn-boundary root and machine-word inventories; no GC-owner witness can replace them |

The canonical-key change is necessary before excluding native metadata: large tenured keys
previously entered the factory's candidate map despite being retained by the key table. A
canonical key is independently owned even when its spelling was produced by a factory. The
receiver's existing key preflight/rewrite protocol still controls destination ownership.

Opaque-context inventory allocation failure permanently refuses this heap's optimized admission.
It never roots the callable; sweep prunes dead or reused block witnesses. A lost root registration
refuses even without a field write. The independent root pass checks retained module exports.

## Verification scope

- Cheap source guards and compile checks while implementing.
- Strict oracle parity for direct transfer, late/captured/cache aliases, callable slots, CommonJS
  registration lifetime, and clear/retry behavior.
- Fixed candidates with independently growing unrelated objects, dynamic keys and shape counts.
  Measure certification separately from ordinary collector work and end-to-end actor calls.
- Candidate-size and outside-owner-size curves; weak witnesses intentionally inspect current
  owner layouts, so cost is candidate plus possible owners and independent roots, not a promise
  that every proof operation is constant time.
- Ordinary workload performance, collector stress, concurrent realms and the final frozen gate.

This change does not complete bootstrap sharing. Its process-owned blueprint and realm overlays
remain the separate density/startup implementation under issue 08.

## Implementation checkpoint, 2026-09-29

The generated C passes Clang syntax checking. Rust test compilation passes; all eleven source
inventory guards and fourteen focused candidate fixtures pass, including registration revocation
without a heap write. The large-key fixture confirms that a 64 KiB canonical spelling allocated
inside a factory is absent from the candidate large-block map. These are focused results, not
public actor scaling or collector-stress acceptance.

A second Clang pass follows output parameters transitively through 2,236 function bodies in the
previous frozen compiler input. It conservatively reports 340 addressed member arguments. The
heap outputs are owner-aware construction, explicit post-construction pointer publication, or
common store helpers. The other reviewed arguments are native metadata, private arena state,
local builder outputs and input argument vectors. This pass does not prove arbitrary pointer
alias analysis or macro coverage; lexical guards, generated-source guards and strict oracle
execution remain separate evidence. The new weak-context and facade fields are reviewed in the
current source rather than assumed covered by the earlier compiler input.

`bench/concurrency/js/actor-transfer-scaling.mjs` adds a public fixed-candidate workload with
independent unrelated-object, dynamic-key and shape-path growth. Its rows explicitly include
collection and message delivery; they cannot be relabeled certification-only timings.


## Public scaling checkpoint

Three interleaved rounds of the same native-release binary, with 31 handoffs and three warmups
per process, measured these median milliseconds per handoff at 100,000 unrelated sender objects:

| unrelated state | candidate owners | full sender walk |
| --- | ---: | ---: |
| objects | 0.388 | 3.415 |
| distinct dynamic keys | 0.401 | 7.691 |
| distinct shape paths | 0.392 | 11.388 |

The optimized medians at zero unrelated objects were 0.398–0.410 ms. Every row verified the
moved answer and the held sender checksum. Strict oracle runs agreed for every mode at 0,
1,000, 10,000 and 100,000 objects. Raw rounds, binary hash, source commit and explicit exclusions
are in `performance-evidence/graph-ledger-scaling-2026-09-29.json`. This is local end-to-end
scaling evidence; it is not certification-only timing, an ordinary-workload regression gate or
quiet-machine final performance acceptance. Additional candidate-size and outside-owner-size curves remain future characterization.
Collector stress, concurrent realms and final shipping acceptance are recorded below.

## Shipping verification

The final frozen batch at `6e9cc7a8` passed the workspace, native parity, routine Test262,
thirdparty correctness and conformance lanes. The original gate failed only its dayjs timing;
the isolated performance lane recheck passed all 12 present bundles without changing the
baseline, source or CLI. Full collector stress matched 318 probes with zero divergences.
Strict oracle parity matched eight probes; repeated realms matched those eight at counts
1, 2, 16 and 64, and a three-probe stressed subset at 1, 2 and 16. Each realm count ran twice.
The source, index and binary hashes stayed frozen through verification. Exact counts, original
failed aggregate, accepted lane composition, log hashes and limits are retained in
`performance-evidence/graph-ledger-shipping-verification-2026-09-29.json`.

Broad bootstrap sharing remains deferred by user decision; its density and startup targets
have not been reached. Additional size curves do not change the proof's candidate-plus-owner
and independent-root complexity contract.
