# Issue 03: the inventory measures against node

Status: ready
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
