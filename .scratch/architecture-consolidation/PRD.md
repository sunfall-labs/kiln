# PRD: Architecture consolidation

Status: needs-triage
Opened: 2026-09-26

## Problem

The architecture's core decisions are sound: one bytecode feeding two engines (ADR 0061), one owner
per semantic operation (ADR 0060), built-ins in TypeScript (ADR 0005), and refusals instead of
silent `undefined` (ADR 0063). What undermines them is execution. Measured on 2026-09-26:

- **Semantics are still duplicated.** The core intrinsics (String, Array, Object, Promise) have
  about 600 interpreter `Native` variants and 470 C method codes: `String.prototype.padStart` is
  `kiln-interpreter/src/lib.rs:38866` and `kiln-backend-c/src/runtime/strings.c:1245`.
  `EventEmitter` exists three times (`events.rs`, `runtime/node.c`, `node/events.ts`).
- **The TypeScript migration adds instead of replacing.** From 2026-09-05 to 09-26 the bootstrap
  TypeScript grew 0 → 14.5k lines while the C runtime grew 33k → 45k and interpreter `lib.rs` grew
  46k → 50k.
- **The seam between the TypeScript built-ins and the engines is undeclared.** Built-ins capture
  public
  members at bootstrap (`date.ts` reads `Date.prototype.getTime`) or look up host functions by
  string on `process.__kilnNativeModules`, which user code can see.
- **Files are too big to iterate on.** `kiln-interpreter/src/lib.rs` is 49,652 lines, and
  `kiln-ir/src/compile.rs` (9.8k), `backend-c/src/graph.rs` (7.2k) and `runtime/dispatch.c` (7.1k)
  are heading the same way. Many sessions edit these at once.
- **Dependencies point the wrong way.** The native backend depends on the interpreter through
  `kiln-module-graph`, so every interpreter edit rebuilds the native backend.
- **Some of the build is invisible to Cargo.** The provider staticlib is built by a script, the C
  prelude is assembled by concatenation in an order that is load-bearing, and about 40 bash scripts
  carry version-specific traps. Much of `CLAUDE.md` exists to warn about these.

## Goal

Every observable behaviour lives in exactly one source file; everything else is generated from it or
is engine-specific heap mechanics. The tree says where code goes, and the tooling makes the wrong
place fail to build.

## Out of scope

- CI changes of any kind (merge queues, scheduled fuzzing, sanitizer runs, performance budgets
  in CI). The local gate (`mise run gate`) is in scope.
- New language or Node surface, except as a side effect of deleting a duplicate.

## Decisions this plan depends on

| ADR | What it decides | State |
| --- | --- | --- |
| 0060 (revised 2026-09-26) | one owner per semantic operation; migration deletes what it replaces; the number of members implemented more than once can only go down | Accepted 2026-09-26 |
| 0059 | intrinsics are ordinary realm objects, built from one description | Accepted 2026-09-26 |
| 0065 | engines implement a declared primitive set; built-ins are TypeScript over it | Accepted 2026-09-26 |
| 0066 | crates belong to layers; dependencies point down | Accepted 2026-09-26 |

Issue 01 settled these on 2026-09-26; nothing in this plan is waiting on a decision.

## Success measures

Reported, not gated in CI:

| measure | 2026-09-26 | direction |
| --- | --- | --- |
| members implemented more than once (the ADR 0060 inventory) | not yet counted | down, never up |
| interpreter `Native` variants | ~600 | down |
| C `KILN_C99_METHOD_*` codes | ~470 | down |
| C runtime lines (`runtime/*.c`) | ~45k | down |
| largest Rust source file | 49,652 lines | under 5k |
| normal dependency edges violating ADR 0066 | 3 | 0 |
| `CLAUDE.md` sections that warn about a trap rather than describe a mechanism | most of the file | down |

The counts marked ~ come from grep and are rough; issue 07 replaces them with the inventory's exact
numbers.

## Order

1. **Settle the ADRs** (01). Unblocks everything that changes ownership.
2. **Cheap, mechanical, unblocking:** split the big files (02, 03), per-crate READMEs (04), and
   housekeeping (18).
3. **Layering** (05, 06): the check, then the inversions, then the directory move.
4. **Deduplication:** the inventory and ratchet (07), the primitive channel (08), then family by
   family, starting with the Node surface (09, 10) and then String (11).
5. **Tooling that keeps it fixed:** `kiln explain` (12), removing panics user code can reach (13).
6. **Build and scripts:** order-independent C prelude (14), provider staticlib via Cargo (15),
   scripts to `cargo xtask` (16), one test entry point (17).

Pure-move changes (02, 03, the directory move in 06) are landed quickly and one at a time, because
several sessions write to `main` at once and a long-lived move branch conflicts with all of them.

## Issues

| # | title | status |
| --- | --- | --- |
| 01 | [Settle the proposed ADRs](issues/01-settle-proposed-adrs.md) | resolved |
| 02 | [Split `kiln-interpreter/src/lib.rs` into modules](issues/02-split-interpreter-lib.md) | ready-for-agent |
| 03 | [Split the other oversized files](issues/03-split-other-oversized-files.md) | ready-for-agent |
| 04 | [A README for every crate](issues/04-crate-readmes.md) | ready-for-agent |
| 05 | [Layer check test](issues/05-layer-check-test.md) | ready-for-agent |
| 06 | [Remove the layer inversions and move crates into layer directories](issues/06-remove-inversions-and-move-crates.md) | ready-for-agent |
| 07 | [Ownership inventory and the duplicate count](issues/07-ownership-inventory-and-ratchet.md) | ready-for-agent |
| 08 | [The primitive table and `kiln:primitives`](issues/08-primitive-table-and-channel.md) | ready-for-agent |
| 09 | [Retire `process.__kilnNativeModules`](issues/09-retire-kiln-native-modules.md) | ready-for-agent |
| 10 | [`node:events` has one implementation](issues/10-node-events-single-owner.md) | ready-for-agent |
| 11 | [`String.prototype` in TypeScript over primitives](issues/11-string-prototype-over-primitives.md) | ready-for-agent |
| 12 | [`kiln explain <member>`](issues/12-kiln-explain.md) | ready-for-agent |
| 13 | [Remove panics that user code can reach in the interpreter](issues/13-interpreter-panics.md) | ready-for-agent |
| 14 | [Make the C prelude order-independent](issues/14-c-prelude-order-independent.md) | ready-for-agent |
| 15 | [Build the provider staticlib through Cargo](issues/15-provider-staticlib-via-cargo.md) | ready-for-agent |
| 16 | [Move the scripts to `cargo xtask`](issues/16-scripts-to-xtask.md) | ready-for-agent |
| 17 | [One test entry point](issues/17-one-test-entry-point.md) | ready-for-agent |
| 18 | [Housekeeping](issues/18-housekeeping.md) | ready-for-agent |
