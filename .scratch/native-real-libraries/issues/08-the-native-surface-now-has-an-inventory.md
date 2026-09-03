# The native surface now has an inventory, and it says where to start

Status: ready

`scripts/native-surface-inventory.py` asks both engines about every member of every built-in, every
prototype and every `node:` namespace, and writes `../native-surface-inventory.md`. Re-run it after
a backend change and diff the report; nothing in it is hand-written, and the tool names no built-in
of its own, so a member added to either engine turns up in the next run without anyone editing a
list.

**At `1f1ea59f`, of 1000 members across 148 receivers: 57 diverged, 305 refused, 638 implemented**,
plus 51 names published natively that the interpreter does not have at all. 558 native compiles,
twelve minutes, no run that failed for a reason that was not a refusal.

The report carries the commit it was measured at, in its own second paragraph. A surface inventory
without that rots quietly, and a stale sweep file misled someone badly on the same day this was
taken.

## What moved since `8aec5b8f`

| Class | `8aec5b8f` | `1f1ea59f` |
| --- | ---: | ---: |
| diverged | 101 | **57** |
| refused | 349 | **305** |
| implemented | 550 | **638** |
| invented *(memo)* | 50 | **51** |

Eighty-eight members moved out of the two missing classes. Almost all of that is the `node:`
namespaces, and **it did not move the way the last report predicted.**

## The `node:` class moved from diverged to *implemented*, not to refused

At `8aec5b8f` the `node:` namespaces were 58 of the 101 divergences, 55 of them reading
`undefined`: the namespace published the interpreter's whole export-name list and filled only the
slots it had. The fix on `main` was to stop publishing names the backend cannot answer, and the
expectation was that most of those 58 would come back as honest refusals.

They did not. Of 116 `node:` members: **11 diverged, 6 refused, 99 implemented.**

    node:os        2 of 20 implemented  ->  20 of 20
    node:fs        2 of 19              ->  19 of 19
    node:path      4 of 13              ->  13 of 13
    node:buffer    1 of  8              ->   1 of  8   (unchanged)

Only six `node:` members refuse on the *read*, and all six are a whole namespace the backend does
not materialise at all: `node:zlib` (3) and `node:string_decoder` (1), plus one name each on
`node:assert` and `node:events`.

**"Implemented" here means the two engines agree on `typeof`, which for these members is not the
same as the member working.** The refusal moved to the *call*, and this inventory never calls
anything. Measured, one probe each:

    require("os").platform          typeof "function" in both engines
    require("os").platform()        kiln: native ... operation=CallMethod(platform) ... refused

    require("path").dirname("/a/b/c.css")   -> /a/b      (right answer, natively)
    require("path").join("/a","b","../c")   -> /a/c      (right answer, natively)

So `node:path` is genuinely implemented and `node:os` is genuinely not, and this report puts both
in the same column. That is the honest reading of the number and it should be carried with it:
**the 99 is an upper bound on what works, not a count of it.** `conformance/native` is where a
present-but-wrong member is caught, and `conformance/native/node-path-unbuilt-members-refuse.cjs`
is the probe that pins the new shape down.

This is still the right trade. A member that is callable and refuses when called stops the program
at the cause and names the member; a member that reads `undefined` lets a feature guard pass and
carries a bad value four frames before anything notices, which is what `postcss` cost. The change
converted the second failure into the first. It did not implement 88 members, and the count reads
as though it did.

### `node:buffer` is the one namespace the fix did not reach

Seven of its eight exports --- `alloc`, `allocUnsafe`, `allocUnsafeSlow`, `byteLength`, `concat`,
`from`, `isBuffer` --- are still *published, enumerable and `undefined`*. It is now the only
`node:` namespace in that state, which makes it a one-namespace job rather than a class, and it is
the highest-value single fix on the page: `Buffer.from` is the first thing a bundle reaches for.

## Four names are a third of everything missing

`length` is missing from 74 receivers, `name` from 20, `constructor` from 15, `prototype` from 15
--- **124 of the 362 missing members, 34%.**

Keep these separate from the rest of the backlog. They are function-object metadata that almost
nothing feature-detects on, which is a different *kind* of gap from a missing `readFileSync`: they
are one decision about what a built-in *is*, not 124 omissions. Counting them in makes the backlog
look 52% larger than the part of it a real program will hit, and it distorts the ordering, because
nothing else on the page is 74 receivers wide.

Thirteen of the 74 `length` misses and nine of the 20 `name` misses are *divergences* rather than
refusals: the read answers, and answers wrongly.

    console.log(Object.length, Array.length, AbortController.name);

    interpreted  1 1 AbortController
    native       undefined undefined anonymous

ADR 0043 says `Object.getOwnPropertyNames` omits `length` from a built-in's list "because reading
it refuses, and a name in that list which a read cannot answer is the same lie in the other
direction". That premise no longer holds on 13 receivers, so on those the conclusion has stopped
protecting anything: the list omits the name *and* the read answers `undefined`, which is the lie
the ADR set out to prevent. Either the read should refuse again or `length` should answer; the two
halves have to agree, and on 13 receivers they do not.

## The invented names are drift in the opposite direction

51 names are published natively that the interpreter does not have. This deserves its own line
rather than a footnote, because by ADR 0043's logic it should be exactly as impossible as the
reverse --- one table, two emitters, a `const` assertion when they disagree. It is measured, it is
in the other direction, and nothing catches it: no per-name read would ever find one, because
nothing would think to ask.

Three groups:

- **18 × `default` on a `node:` namespace.** The CommonJS projection of an ESM namespace. Arguably
  correct and arguably the interpreter's gap; decide which, because it is 35% of this class.
- **23 on the error prototypes** --- `hasOwnProperty`, `toString`, `valueOf` on each of
  `EvalError.prototype` and its five siblings, and two on `Error.prototype`. These are inherited
  members that the native side publishes as *own*, which is a shape bug, not a missing member.
- **10 that are wrong by name**: `Map.prototype.add` and `WeakMap.prototype.add` (that is `Set`),
  `Set.prototype.get`/`set` and `WeakSet.prototype.get`/`set` (that is `Map`),
  `BigInt64Array^.apply`/`bind`/`call`/`constructor`, `Array.prototype.toLocaleString`. The
  collection pairs look like one table row copied between two kinds; that one is worth reading the
  emitter for.

56 of the 148 receivers cannot describe themselves natively at all, so an invented member on any of
those is invisible to this method and the 51 is a floor.

## Then the receivers, worst first

Divergence outranks refusal at every size, because a refusal stops a program at the cause and a
wrong answer does not stop it at all.

    ArrayBuffer.prototype       12 of 12 diverged   the receiver itself reads `undefined` natively
    node:buffer                  7 of  8 diverged
    Symbol.prototype             4 of  4 diverged   same shape: `Symbol.prototype` is `undefined`
    Uint8Array.prototype         4 of  6 diverged   the base64/hex methods
    BigInt64Array.prototype^    35 of 35 refused    %TypedArray%.prototype, reachable by no name
    Date.prototype              32 of 47 refused
    String.prototype            19 of 51 (2 diverged)
    globalThis                  11 of 93 refused
    RegExp.prototype            10 of 14 refused

`ArrayBuffer.prototype` and `Symbol.prototype` are one bug each and thirteen and five rows
respectively: reading `.prototype` off those two constructors answers `undefined` natively, so
every member on them is unreachable and would throw "undefined is not an object" at run time. They
are cheap and they are at the top of the list for that reason.

## Two things outside what this method can see

**`fs.existsSync` answers `false` natively where the interpreter refuses at the host layer.** Found
by hand while checking what the `node:fs` 19-of-19 meant; not by the inventory, which never calls
anything.

    kiln --one-shot --node run --native   ->  existsSync false
    kiln --one-shot --node run            ->  host operation failed: node:fs existsSync is
                                              unavailable: filesystem I/O is Tokio-owned and
                                              asynchronous

One engine says "no such file", the other says "I cannot do this". A program that branches on
`existsSync` takes a different path under each, silently. This is the `node:` wrong-answer shape
surviving the fix, one level down from where the fix was applied.

**A run that failed for a reason that was not a refusal used to be filed as one.** In this run
`WeakRef.prototype` hit `cache failed: could not install cache artifact ...` --- the tool handing
its own build scratch back while another receiver was still compiling --- and both its members
went into the *refused* column. Two of a thousand, and nothing would have caught it by reading.
The tool now treats a `cache failed:` line as a run that did not happen: retried, and if it never
recovers, named under "Runs that were not measurements" rather than folded into the backlog. The
two members were re-measured directly afterwards and are genuine refusals --- `WeakRef.prototype`
refuses on the read of `.prototype` itself --- so the 305 stands, but it stood by luck.

## How to re-take this

    mbx build --release -p kiln-cli
    scripts/native-surface-inventory.py

Twelve minutes and about 3 GiB of build scratch, which the tool hands back as it goes. It stops
rather than filing a measurement that ended in ENOSPC; if it says so, that is the answer, not a
reason to retry it into a full disk.
