# Issue 03: the inventory measures against node

Status: fixed (2026-09-19)
Feature: full-runtime
Governing ADR: ADR 0063

`scripts/native-surface-inventory.py` asks the interpreter and the native backend and reports the
difference. That is the near gap. Node is the target of both engines, and nothing enumerates
node's surface the same way, so a member the interpreter invents (`Promise.allKeyed`) or misnames
(`%TypedArray%.name === "intrinsic"`) is invisible to the tool, and a member both engines lack is
counted only if the interpreter happens to have it.

Done when the enumeration stage also runs under the pinned node (the version `bench/` and
`scripts/thirdparty.sh` compare against), every row carries a node column, the report counts
"differs from node" per receiver for each engine beside "refused", and issue 02's baseline
carries those counts too.

## Outcome (2026-09-19)

Landed: the same enumeration runs under the pinned node, every receiver carries node's answers,
and the report's `## Against node` section counts *missing*, *differs* and *extra* per engine,
ranks receivers by distance from node, lists the receivers the interpreter does not publish (81,
517 members -- the native backend is asked about each; `URL.prototype` is one it has), and lists
what the interpreter publishes that node lacks (161 members). Node-only names are put to the
native backend where its own key list claims them, and an absent receiver is read as such from
its `typeof` rather than retried as a tool failure.

First measurement, at `c33ce379` plus this change, node v25.9.0: of 2,291 members node has on
shared receivers the interpreter is missing 1,012 and the native backend 1,212; the interpreter
differs on 10 and native on 14. The near gap (interpreter versus native, the classes above it)
is 17 diverged and 313 refused of 1,721; the far gap is three to four times that. The receivers
farthest from node are `node:zlib` (111 of 154), `Buffer.prototype` (all 96: the interpreter's
`Buffer` has no prototype object), `node:crypto` (63 of 73), `globalThis` (58 of 140), `node:fs`
(53 of 104), `node:module` (27 of 36), `node:util` (21 of 34).

Not yet: the per-receiver baseline (issue 02) should carry these counts too, and at 926 native
compiles a run is three minutes on a warm cache -- `gate-quick` (issue 01) will want `--only`
or a cached enumeration.
