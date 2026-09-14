# ADR 0062: Liveness plans native-provider capabilities

## Status

Accepted.

## Context

The provider was first packaged in two forms distinguished by an `http` boolean. That bit initially
meant exactly what it said, then came to distinguish a common Realm/RegExp/BigInt/JSON archive from
an archive also containing Temporal, Intl, parallel execution, FFI and HTTP. The compiler selected
between those packages with a second full/common boolean.

That representation lost the reason a program needed the larger package, made another variant a
new branch through packaging and caching, and let a manifest imply capabilities it did not name.

## Decision

Provider requirements are a typed set computed from the retained executable graph by
`kiln-liveness`. Each resolved builtin, retained global/member and open-world construct contributes
capabilities. Dynamic imports and CommonJS currently contribute the complete set.

`kiln-native-provider-contract` owns the capability vocabulary, its canonical order, the packaged
variants and each variant's supplied set. A provider manifest declares both its variant and complete
capability list. Validation rejects noncanonical lists and any archive that does not cover the
program's required set. Cache identities contain the variant and capability list. Build reports
record required and supplied sets separately.

The two existing packages remain implementation choices: common and full. The resolver selects the
smallest available variant by set inclusion. Their existence is not exposed to liveness, so a later
archive containing independently removable capability objects or additional packaged variants does
not change program analysis.

## Consequences

- `http` is no longer provider identity or a proxy for unrelated engines.
- A program needing only FFI and one needing only Intl have distinct plans even while both resolve
  to the current full archive.
- Dead modules do not enlarge the plan; capability inference is scoped to live graph nodes.
- Open-world programs remain safe by requesting all capabilities.
- Provider schema 3 is intentionally incompatible with earlier manifests.
- Capability sharding is packaging work behind this contract, not another liveness redesign.

## Rejected alternatives

**Add more booleans to the manifest.** Independent flags still leave selection and cache identity as
parallel hand-maintained logic and do not define a canonical set.

**Package every capability combination.** Nine capabilities already imply an unbounded maintenance
matrix as engines are added. Selection by coverage permits a small, measured set of useful variants.

**Let packaging infer requirements from missing symbols.** Link failures name adapter symbols rather
than program semantics and cannot explain why an engine was retained. Liveness already owns the
closed-world information needed to make the decision explicitly.
