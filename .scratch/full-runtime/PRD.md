# PRD: Full runtime — the native surface converges on the interpreter's

Status: ready
Governing ADR: ADR 0063 (a refusal is a debt), ADR 0062 (demand-driven provider capabilities),
ADR 0005 (built-ins in TypeScript), ADR 0025 (engines are linked)

## Why

kiln is a runtime in place of node, and node is the target of both engines; the interpreter
leads the native backend and is its differential authority, not the destination. A program that
stops at `Intl.NumberFormat`, `crypto.randomUUID` or
`date.toLocaleDateString()` with a message naming the member is not running, however honest the
message. ADR 0063 makes every native refusal a debt with a baseline that only goes down. This PRD
is the backlog in the order that ADR sets, with the numbers it starts from.

At `047bc973` (2026-09-19, `scripts/native-surface-inventory.py`): 1721 rows, 1391 implemented
(80.8%), 313 refused (18.2%), 17 diverged, 0 silent `undefined`.

Against node (v25.9.0), measured the same day once the inventory could ask it: of 2,291 members
node has on the receivers both share, the interpreter lacks 1,012 and the native backend 1,212,
and node has 81 receivers (517 members) the interpreter does not publish at all. The near gap
above is a quarter of the far one.

## Gate first (issues 01, 02, 03)

1. `native-surface-inventory.py --gate` in `gate-quick`. Two minutes on a warm cache. The class
   it fails on -- `undefined` where the interpreter has a value -- is the one that costs weeks,
   and it went 1 -> 111 in sixteen days with nothing running it.
2. A per-receiver refused-count baseline, committed beside `conformance/test262-baseline.json`
   and compared the same way: a receiver whose count rises fails the gate and names itself. The
   whole-corpus number hides a receiver gaining ten while another loses ten.
3. A node column in the inventory: the same enumeration run under the pinned node, so every row
   reads interpreter / native / node, and "differs from node" is counted per receiver beside
   "refused". Without it the tool measures the near gap only.

## The backlog, in order

Demand first: whatever `scripts/thirdparty.sh`'s refusal distribution names, as bundles are added
(Track C). At 17/17 matched today it names nothing, so the inventory's order applies:

| # | receiver | refused | how (ADR 0063 rule 4) |
| --- | --- | ---: | --- |
| 03 | typed-array prototype (`BigInt64Array.prototype^`, shared by all eleven) | 41 | TypeScript over the element intrinsics the ambient prelude already has |
| 04 | `Date.prototype` | 33 | TypeScript over the instant; `toLocale*` through Intl (06) |
| 05 | `String.prototype` + `RegExp.prototype` | 15 + 11 | TypeScript; the regexp engine is linked already |
| 06 | `Intl` | 12 | ICU4X on demand through ADR 0062 capabilities, reversing ADR 0042's deferral |
| 07 | `crypto` | 12 | `node:crypto` linked when liveness sees the global read; the refusing namespace goes |
| 08 | `DisposableStack`, `AsyncDisposableStack` | 22 | TypeScript; `Symbol.dispose` landed 2026-09-19 |
| 09 | `ArrayBuffer.prototype`, `SharedArrayBuffer.prototype` | 18 | resizable/transfer surface in TypeScript over the buffer intrinsics |
| 10 | `Set.prototype` set-algebra, `FinalizationRegistry`, the eleven on `globalThis` | 25 | TypeScript |
| 11 | the twelve subclass-prototype `name` rows answering `"Error"` | (diverged) | one prototype object per error kind |
| 12 | `ShadowRealm` | 11 | ADR 0038 realms; the one `conformance/native` refusal probe |

Each row lands with its receiver's baseline number moved down in the same diff, the way a
Test262 chunk does.

## What app-shaped programs hit (2026-09-19)

Five twenty-line programs, each what a real project does in its first minute, run under node,
the interpreter and native (`.scratch/full-runtime/probes/`, the green ones now in
`conformance/native/app-*`). Everything they exposed was fixed the same day except two:

* `Buffer.prototype` in neither engine (issue 08) -- `events-streams.mjs` is node-identical
  on every other line.
* The interpreter exits 0 on an unsettled top-level await where node exits 13 (issue 09).

Fixed from the same probes: `Readable.from` never flowed and `Transform` never ended;
`events.once`/`on`; `fs.rmSync`/`mkdtempSync` composed in TypeScript when the host lacks
them; the native `u` flag (the literal compiler compiled `\p{L}` as the letter `p`) and both
engines' non-`u` `.` matching a surrogate pair as one character; `normalize()` the identity in
both engines and `localeCompare` code units natively (now ICU4X through `kiln-intl` for
both); `toUpperCase` beyond ASCII natively; `["é"].join("")` natively (Latin-1 bytes copied
into a UTF-8 builder); `nextTick` ordering after an ES module body (node runs the body inside
a promise job, so the promise queue drains before the tick lane); `structuredClone`,
`Object.groupBy`, `Map.groupBy`; a typed-site declaration missing from modules with
top-level await; the overlay host's `rmSync` leaving in-memory entries behind.

## The interpreter's own drift from node (issues 05, 13)

Surface the interpreter has and node does not, or answers differently, is the same debt on the
other engine: `Promise.allKeyed` / `allSettledKeyed`, `%TypedArray%.name === "intrinsic"`, the
`Console` prototype shape, `console.table` printing as `log`. Issue 03's node column is what
makes this measurable rather than noticed one member at a time.
