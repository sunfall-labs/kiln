# ADR 0059: Intrinsics are ordinary realm objects built from one description

## Status

Accepted, 2026-09-26. Decided by the project owner with ADR 0060 and ADR 0065, as the object-model
half of giving each built-in member one owner.

Originally proposed, 2026-09-13.

Extends ADR 0005 (built-ins are TypeScript bootstrap code), ADR 0038 (a realm owns its
intrinsics), ADR 0043 (the built-in surface is declared once) and ADR 0054 (a built-in member is
described once). Supersedes ADR 0038's marker-and-retagging representation and ADR 0043's statement
that a member's behaviour stays separately implemented in each engine, but only when the migration
described here reaches that member.

## Context

An intrinsic prototype in the interpreter is currently a `Native::PrototypeOf` marker rather than
an ordinary `ObjectData`. Reads, own-property queries, enumeration, attributes, accessors,
deletion, `name` and `length` therefore travel through related but distinct tables and special
paths. ADR 0054 made several kinds of disagreement unrepresentable, but records the remaining
end state itself: materialise intrinsics as real objects so ordinary property semantics answer all
of those questions.

The native backend has the same architectural problem in a different representation. A built-in
member can be present in the shared surface, dispatched by a method code, described by reflection,
and exposed by the TypeScript bootstrap through separate mechanisms. Compile-time completeness
checks turn some drift into a build failure, but the number of descriptions is still greater than
one.

This is not only a maintenance concern. Intrinsic mutation, deletion, realm identity, descriptors
and symbol-keyed members are observable ECMAScript semantics. A special path that almost behaves
like an object is a second object model.

## Decision

**Each intrinsic constructor and prototype is an ordinary realm-owned object, populated from one
typed member description.**

The description is representation-independent and contains, for each member:

- its owner and typed key (`Named` or a well-known symbol);
- whether it is a data property, method, getter or setter;
- its descriptor attributes;
- the callable's JavaScript `name` and `length`, when applicable;
- its authoritative semantic binding;
- whether an engine implements it, deliberately refuses it, or has a named migration backlog.

Realm construction consumes the description and defines properties through the same object
machinery used by program-created objects. An engine may choose eager construction, lazy
construction on first access, or a frozen heap image, provided the choice is not observable and
the resulting value is an ordinary object after materialisation.

The description is the input to generated engine adapters, TypeScript declarations where useful,
surface completeness checks and reflection tests. Generated output may differ by representation;
the declared member and its semantic owner may not.

The ordinary object path owns reads, writes, deletion, enumeration, descriptors and prototype
walking after materialisation. `IntrinsicSlots`, intrinsic-key switches, deletion tombstones and
marker retagging are migration mechanisms, not permanent architecture.

Migration is member-family by member-family. Until a family is migrated, the existing mechanisms
and their compile-time gates remain authoritative. A family is complete only when both engines
materialise it from the description and the superseded special paths for that family are deleted.

## Implementation mechanism

The authoritative description is ordinary typed Rust data in the dependency-free
`kiln-builtin-surface` layer, extended or replaced in place rather than shadowed by a second
schema. It begins as visible `const` rows and validation functions. Existing `serde` support may
write a stable generated manifest when another tool needs one, but a serialised file is an output,
not a second source of truth.

A small repository-owned generator consumes the rows and emits the representation-specific
construction tables, completeness tests and TypeScript declarations. Generated files are
deterministic, committed when they are part of a shipped or reviewed interface, and checked for a
clean regeneration in the gate. Code generation does not happen through a proc macro until the
schema has stabilised and a measured compile-time or maintenance cost justifies hiding the
expansion.

`inventory` and `linkme` are not used for member registration. Distributed linker registration
would make the complete intrinsic surface difficult to inspect and would weaken the rule that one
table is the work queue. `phf` is not adopted pre-emptively; generated perfect hashing is an
available representation optimization only if lookup or realm construction is measured as a
material cost.

The description derives or implements `arbitrary::Arbitrary` in the fuzz workspace where useful.
`proptest` generates valid and invalid descriptions and checks construction, reflection,
descriptor, deletion and symbol-enumeration invariants. These libraries generate test inputs; they
do not define the production schema.

## Invariants

1. A member is described in one row or appears in a named backlog with a reason.
2. A symbol key is a type, never a reserved string spelling in the declaration.
3. Existence precedes description: no attribute or behaviour rule can invent an absent member.
4. A realm creates distinct intrinsic object identities while sharing only the state the
   specification says agents share.
5. Mutating or deleting an intrinsic uses ordinary object semantics and is immediately visible to
   every lookup and optimization guard in that realm.
6. Closed-world specialization may bypass a lookup only under the epoch, shape or proof required
   to preserve invariant 5.

## Consequences

- Reflection and ordinary property access no longer have separate sources of truth.
- ADR 0038's realm semantics survive, but realm identity comes from object identity and prototype
  links rather than a `RealmId` carried by intrinsic markers.
- ADR 0043's shared surface and completeness checks become inputs to the richer description.
  Its permanent two-behaviour model is superseded as families migrate.
- ADR 0054's typed keys and one-row discipline are retained; its marker tables and tombstones are
  deleted when no migrated family uses them.
- Realm startup may become more expensive. Lazy materialisation or a verified heap image may be
  adopted only after measuring process-per-test startup and shipped-program startup separately.
- The generated C and interpreter object layouts remain different. This decision unifies the
  declaration and observable semantics, not their heap representations.

## Rejected alternatives

**Keep markers and add more consistency sweeps.** Sweeps detect known relationships after they
have been encoded twice. Ordinary objects remove the duplicate relationship.

**Share only member names.** ADR 0043 already does this and explicitly records that it does not
make the engines agree on behaviour, descriptors, or identity.

**One process-global intrinsic object graph.** This violates ADR 0038 and ECMAScript realm
identity even if it improves startup.

**Distributed registration through `inventory` or `linkme`.** This makes extension convenient at
the cost of hiding completeness and ordering behind the linker. Intrinsics are a closed language
surface, not a plugin registry.
