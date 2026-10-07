# 10. `node:events` has one implementation

Status: ready-for-agent
Blocked by: 07, 08

`EventEmitter` exists three times: `crates/semantics/kiln-builtins/src/node/events.ts`,
`crates/interp/kiln-interpreter/src/events.rs`, and the event-emitter section of
`crates/native/kiln-backend-c/src/runtime/node.c`. Make the TypeScript the only one, per ADR 0060's
delete-on-migrate rule.

First find out what the Rust and C copies are still used for. The likely answer is the objects the
engines create themselves (HTTP servers, sockets, streams) that must be emitters before any
TypeScript runs; those become TypeScript classes the host constructs through a primitive, or the
TypeScript installs the prototype they inherit from.

This is the first family to go through the whole ADR 0060 / 0065 path, so write down what was
awkward in this issue's comments; 11 relies on it.

## Acceptance

- `events.rs` is deleted and `node.c` has no emitter implementation.
- The inventory's duplicate count for `node:events` is zero and the baseline is lowered in the same
  change.
- `kiln test conformance/node --node`, `conformance/server --node`, `scripts/native.sh` and
  `scripts/thirdparty.sh` pass.
