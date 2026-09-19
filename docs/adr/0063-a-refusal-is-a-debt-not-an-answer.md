# ADR 0063: A refusal is a debt, not an answer

## Status

Accepted, 2026-09-19. Decided by the project owner after the native surface inventory of that
day; this ADR records the decision and what it changes.

Amends ADR 0042 (`Intl` refused by name) and the fourth rule of ADR 0043 ("a built-in is
converted, or it is backlog"): both stay as the record of *why* a refusal was the right thing to
emit on the day, and neither is any longer a licence to leave one in place.

## Context

kiln is meant to be a runtime -- the thing a program is shipped on, in place of node -- not a
compiler that handles a subset and says so. Node is the target: the thirdparty corpus is held to
byte-identical output against it, the shared surface table cites its ordering, and a program
written for node is the program kiln exists to run. The native backend's discipline of refusing what it has not built (ADR 0017,
ADR 0042, `AGENTS.md`) was adopted for a real reason: an `undefined` where node has a value is a
wrong answer nothing stops at, and a bundled `postcss` died on one for weeks. A refusal is the
honest report of a gap. That reasoning is right and stays.

What drifted is what a refusal was allowed to *mean*. ADR 0042 argues that "refusing is not the
same as being absent ... neither is a wrong answer", and concludes that turning a refusal into an
answer is "real, and not worth 23 MiB on every artifact before anything asks for it". Read as a
correctness rule, that is sound. Read as a product rule, it says a program that calls
`Intl.NumberFormat` -- or, as of this morning, `console.debug` -- may stop with a message, and
that this is an acceptable end state. It is not. A runtime that stops at `crypto.randomUUID`
until the program happens to import `node:crypto` is not a runtime anyone ships on, however
clearly it names the member.

The 2026-09-19 inventory (`scripts/native-surface-inventory.py`, at `047bc973`) puts numbers on
where the native surface stands against the interpreter's. That is the *near* gap -- the
interpreter leads and the native backend follows it -- and it is the one the tool measures today;
the far gap, either engine against node, is not yet measured by anything but the corpora:

| Class | Rows | Share |
| --- | ---: | ---: |
| implemented | 1391 | 80.8% |
| refused by name | 313 | 18.2% |
| diverged | 17 | 1.0% |
| silent `undefined` | 0 | -- |

The 313 refusals concentrate in a few receivers: `Date.prototype` (33), `BigInt64Array`'s
inherited typed-array prototype (41), `String.prototype` (15), `Intl` (12), `crypto` (12),
`RegExp.prototype` (11), the two `DisposableStack` prototypes (11 each), `globalThis` (11),
`ArrayBuffer.prototype` (9), `SharedArrayBuffer.prototype` (9), `Set.prototype` (8). Every one
of them is a member the interpreter answers.

Three things the day made plain:

1. **A refusal that stands long enough turns into the wrong answer anyway.** And the
   interpreter is not exempt: it publishes `Promise.allKeyed`, names `%TypedArray%`
   `"intrinsic"`, and had six constructor arities node does not -- surface node lacks or answers
   differently, which a program written for node will feel exactly as it feels a refusal. The 09-03 inventory
   had 1 silent `undefined`; by 09-19 it had 111. Thirty-one of them were refusals that had
   *become* `undefined` -- an `Error.prototype` site that named its four members and checked none
   of them, and a catch-all in `kiln_c99_get_key` that converted every refusal on a well-known
   symbol into `undefined` so the runtime's own probes would not stop. The catch-all's original
   comment said "THIS IS A CATCH-ALL AND NOT A LIST, WHICH MAKES IT THE ONE ABSENCE HERE THAT CAN
   LIE" and asked for the fix upstream; the comment was lost in a rewrite and the lie stayed.
   A refusal is honest on the day it is written. Left in place, it is one refactor from silence.

2. **The size argument decides the mechanism, not the surface.** ADR 0042's 23 MiB is a cost of
   linking ICU4X into *every* artifact. ADR 0062 has since made provider capabilities a typed set
   that `kiln-liveness` computes from the retained graph, with the packager supplying the smallest
   archive that covers it. A program that never reads `Intl` links no ICU4X; one that does links
   exactly what it reads. The same mechanism answers `crypto`: a global whose members refuse until
   a module is imported is a linkage decision surfacing as a language one, and liveness already
   knows whether `crypto.randomUUID` is read.

3. **The order is decided by demand, and demand is measurable.** Track C of the plan of record
   already says the thirdparty corpus's refusal distribution -- sorted by how many bundles each
   refusal unblocks -- decides what the runtime builds next. The inventory gives the same for the
   language surface, per receiver. Neither has been the schedule; both are now.

## Decision

**1. Both engines converge on node's surface. A refusal is a gap with an owner and a direction,
never a product behaviour.** Node is the target; the interpreter is the leading implementation
and the differential authority for the native backend (ADR 0020, ADR 0060), not the destination.
A member node answers is a member both engines answer, the same way; a member the interpreter
answers and node does not is debt on the interpreter's side. A refusal is the honest report of
work not yet done -- honest precisely because it is temporary. `conformance/native/
*-refuses.ts` records *that* something refuses today so nobody "fixes" it into an `undefined`;
it does not record that refusing is the design.

**2. The refused count is a baseline that only goes down, and the inventory measures against
node.** `scripts/native-surface-inventory.py --gate` (no silent `undefined`) joins `gate-quick`.
The inventory gains a node column -- the same enumeration run under the pinned node, so every row
reads interpreter, native, node -- and its per-receiver counts of refused and of
differs-from-node are committed beside the Test262 baseline and compared the same way: a receiver
whose count rises fails the gate and names the receiver, exactly as a Test262 chunk does. Moving
a number down is the point; moving one up is a diff a reviewer has to accept.

**3. Order by demand.** The thirdparty corpus's refusal distribution first, because a refusal a
real bundle hits stops a real program; then the inventory's receivers by refused count, largest
first, because a whole prototype missing is one decision made fourteen times. Today that order
is: the typed-array prototype and `Date.prototype`; `String.prototype` and `RegExp.prototype`;
`Intl` and `crypto` through ADR 0062 linkage; `DisposableStack`, `ArrayBuffer`,
`SharedArrayBuffer`, `Set`; the eleven on `globalThis`.

**4. How a gap closes is already decided; this ADR only removes the option of not closing it.**
Built-ins are TypeScript with intrinsics (ADR 0005, the Phase 5.1 freeze). Semantics that need
an engine link one across a coarse ABI (ADR 0025), demand-driven per ADR 0062 so the artifact
pays only for what it reads. C is for the heap-touching part and nothing else. An artifact-size
cost is an argument about *which* of these to use, and a reason to make linkage lazier, not a
reason to refuse.

**5. Consequently, ADR 0042's deferral is scheduled, not open, and so is the interpreter's own
drift from node.** `Promise.allKeyed` and its sibling come out or are recorded as a deliberate
extension with a reason; `%TypedArray%` is named as node names it; an arity is node's arity. `Intl` links ICU4X on demand.
The measurement in ADR 0042 stands as the reason it must be demand-driven rather than
unconditional. The same applies to `crypto` (the refusing namespace installed on 2026-09-19 is a
step on the way, not a destination), to `Temporal` behind its retain group, and to every backlog
row in `BUILTIN_SURFACE_BACKLOG` whose reason begins "not implemented".

## Consequences

- `BUILTIN_SURFACE_BACKLOG` rows and `KILN_C99_REFUSE` sites are debt. A new one is accepted
  with a reason that names why *today*, and it is counted by the baseline from the day it lands.
- The inventory runs in the gate. It costs about six hundred native compiles; on a warm cache
  that is two minutes, which `gate-quick` can carry.
- `AGENTS.md`'s rule that an `undefined` is the expensive answer and a refusal the cheap one is
  still true and still the rule for an individual read. What changes is the sentence after it:
  the cheap answer is the one you may give today, and the baseline says how long "today" is.
- Issue `.scratch/native-real-libraries/issues/09` records what was deliberately left refusing
  on 2026-09-19 (`crypto` until linked, subclass-prototype error names, the `%s` formatting
  native `console` lacks). Under this ADR each of those is backlog with a place in rule 3, not a
  closed question.

## What would reverse this

Nothing short of a decision that kiln is a compiler for a subset. That decision would be its own
ADR, and it would have to say which subset and why a program should learn it from a stop.
