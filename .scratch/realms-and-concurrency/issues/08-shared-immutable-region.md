# 08. The shared immutable region

Status: ready-for-agent
Blocked by: 04

ADR 0070's primitive B. A region of objects that never change, readable from every realm and every
scheduler thread without copying, through a read path that writes nothing. That read path is the
same one ADR 0069 needs (`.scratch/parallel-builtins/` issue 06): build it once, here.

1. **Built-ins and bootstrap first.** Build the intrinsics and the bootstrap heap once per process
   into the region, and have each realm start from it instead of rebuilding it. Where a realm
   changes a built-in (which the language allows), the change is copy-on-write into the realm's
   own heap, so every realm still sees an ordinary, mutable set of intrinsics (ADR 0059).
2. **Then user data**, for 11: freezing a value graph into the region.

A human reviews the design first: it changes how intrinsics are materialised and what the
collector may assume about pointers into the region.

## Acceptance

- The memory of a second realm, measured as in 01, drops by the bootstrap heap's size.
- Modifying a built-in in one realm is invisible to every other, in a probe.
- `mise run native-stress` clean.
