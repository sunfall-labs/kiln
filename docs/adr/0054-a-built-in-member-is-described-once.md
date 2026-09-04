# 0054. A built-in member is described once

## Status

Accepted, 2026-09-03.

## Context

A built-in prototype in the interpreter is a *marker*, not an object:
`Native::PrototypeOf(PrototypeTable::Array)` has no property store. Everything a program can ask
about one of its members is therefore answered by a table, and over time there came to be several
tables per question:

| question | answered by |
| --- | --- |
| does it exist, and what is its value | `Native::property` and the per-table value functions |
| does it enumerate | `intrinsic_prototype_method_names` |
| is it an accessor | `prototype_accessor_names` |
| which getter does it run | `prototype_accessor` |
| what are its flags | `intrinsic_own_attributes` |
| what are its `name` and `length` | `native_js_name`, `native_length` |

Nothing makes these agree. Every disagreement is a silent wrong answer rather than a failure,
because each one is a plausible property that simply reports something different depending on which
question you ask. All of these have been shipped:

- an own `length` that `getOwnPropertyDescriptor` reported and `hasOwnProperty` denied;
- symbol members that read correctly and are absent from `getOwnPropertySymbols`;
- a `delete` that answers `true` and removes nothing;
- and the expensive one: `"@@hasInstance"` -- the *internal* spelling of `Symbol.hasInstance` --
  written into a list of string names, which leaked that spelling into every property listing in
  the realm. The change looked correct. It cost 838 rows in `built-ins/Object` alone, and the
  post-mortem that followed blamed the wrong mechanism, so it was nearly paid for twice.

The shape is always the same: a member is described in more than one place, and the places drift.

## Decision

**A built-in member is described in one row, and the compiler enforces what it can.**

`crates/kiln-interpreter/src/intrinsic_members.rs` holds the descriptions, and three properties of
it are load-bearing:

1. **A key's kind is a type, not a spelling.** `MemberKey::Named(&str)` and
   `MemberKey::Symbol(WellKnown)` are different variants, so a symbol-keyed member cannot be
   written where a string-keyed one is expected. The `@@` convention is an implementation detail of
   *storage*; it is no longer how the two kinds are told apart.

2. **The named lists are checked while the compiler builds them.** Each list is a `const` built
   through `NamedMembers::new`, whose `const fn` body refuses a reserved `@@` spelling. A `const`
   item is evaluated at build time, so the mistake above is now a compilation failure carrying the
   explanation and the fix. This follows the existing `KILN_C99_ABSENT` / `ABSENT_ANSWER_SITES`
   idiom, for the same reason: a rule a reviewer has to remember is a rule that gets forgotten.

3. **A name and its behaviour live in the same row.** `PROTOTYPE_ACCESSORS` carries the table, the
   key and the getter together, replacing the pair of tables that listed accessors and resolved
   them separately. A name present in one and absent from the other used to be a property that
   listed and described as a data property holding `undefined`; it is now unrepresentable.

## Consequences

Adding an accessor is one row instead of two edits in two places, and forgetting the second is no
longer possible. Adding a symbol-keyed member cannot silently corrupt enumeration. The refactor is
behaviour-neutral by construction and was gated as such: every measured chunk identical, native
parity 92/6/0, thirdparty 19/0 on both engines.

### Existence precedes description

Added after the first version of this ADR, because the class it called "narrowed rather than
closed" reappeared within the hour.

`intrinsic_own_attributes` was a run of early returns, each deciding flags from the member's
*name*. Attributes are asked before the value is, so a rule that fires for a name the table does
not publish does not merely mis-describe a property -- it invents one. Generalising a single rule
from `Date` to every prototype gave every prototype in the realm a phantom own `@@toPrimitive`,
which cost `Date` ten rows, `language/expressions` seven and `Object` four, none of them anywhere
near the change. The first fix was to remember an existence check in that rule and the one beside
it, which is the same discipline that had already failed.

It is now structural. `intrinsic_own_attributes` performs the existence lookup **once, before any
rule can run**, and the rules live in `published_attributes`, which returns `Attributes` rather
than `Option<Attributes>`. That function cannot express "and also this does not exist", so a rule
added to it cannot invent a member however it is written.

Three sweeps in `tests.rs` keep the pair honest across every table, rather than wherever someone
happened to look: attributes are never reported for an absent member; the filtered listing and the
description agree in both directions; and no enumeration list carries a symbol's spelling. The
first was checked by reintroducing the phantom on purpose and confirming it names the table and the
member -- a test that has never failed is not known to work.

The raw name lists stay deliberate supersets: `Array.prototype` and `%TypedArray%.prototype` share
one, so the array table names `set`, which only the view has. `intrinsic_prototype_entries` drops
any name the value tables do not answer, so the listing is right even though the list is not, and
the sweep asserts that resolution rather than the list.

### Still not collapsed

`name` and `length` remain in their own functions, so that pair can still drift from the rows. The
rows are the place to put them.

The end state this points at, and deliberately does not attempt here, is materialising the
intrinsics as real objects at realm setup: build an `ObjectData` per prototype from these rows,
lazily on first touch, and every question is then answered by the ordinary object path that is
already correct. That would also delete `IntrinsicSlots`, `IntrinsicKey` and the deletion
tombstones, since deleting a property would simply be deleting a property. It is a realm-construction
change with its own performance question -- this repository runs a process per test -- so it is worth
doing on measurements rather than on principle.
