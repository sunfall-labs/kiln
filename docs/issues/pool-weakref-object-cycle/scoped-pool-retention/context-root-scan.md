# Exact Context-root scan

This remains an opt-in diagnostic, not a Pool conformance result or a collector change. It follows
the retained allocation Scope through its same-run object ancestry, then checks each ancestor's
identity against direct VM, job, timer, async-context, and shared-realm Value roots.

## Provenance

- Probe SHA-256: `dc0e5485b048b7f238d066a790a3681a804c5aa892ba10f5b11629a9f1af90b9`.
- CLI SHA-256: `f5fc9e1d2ccf9f5aa630268df058fdf824be31028f91099dcd92d325fd466836`.
- Source revision: `01688010f114d2086e38a4a392d52b920f2a31d4` plus the private, uncommitted trace
  edits in `cycle_gc.rs`, `weak_refs.rs`, `execution.rs`, `module_map.rs`, and `lib.rs`.
- Full probe log SHA-256: `1f85de11b051ebf397c2be5d5c2b4bc7eba28cbb4fb90ce069328a3d9ae0aa28`.
- Invocation: `KILN_POOL_GC_TRACE=1 <cli> --one-shot run packages/effect/test/.probe/pool-owner.ts`
  from the isolated pinned Effect source workspace.

## Identity chain and query

The log identifies the two retained Scope objects at `POOL_GC_SCOPE_TARGET` and derives their
parents during that same run. Each chain is:

`Scope <- Array.items[1] <- Array.items[0] <- Map-like Object.@@entries <- Context Object.cacheRoot`

The Context candidates are the ordinary objects whose keys include `cacheRoot`, `base`,
`overlay`, `depth`, `_flat`, `baseHits`, and `_fiberCache`; the preceding object has the internal
Map keys `@@entries`, `@@map`, and `@@size`. The recursive object-owner trace visits each parent
object and performs the direct-root identity lookup for that exact object before following its
parents. This avoids treating a query for the Scope itself as a query for its containing Context.

The same-run context identities in this log are `0x7f9d0564ea80` and `0x7f9d2f885ea0` (ASLR
specific). Neither has a `POOL_GC_OWNER_DIRECT_ROOT` match. The expanded direct-root snapshot also
records queued React settled/handler/context/sink values and VM/shared-realm rejection reasons and
process callbacks; none of the logged direct roots matched either Context identity. At the
retention checkpoint the Context and its containing object are both strong and have no incoming
production owner edges. This is evidence that the inspected roots do not directly own the
Context, not proof that every Rust-held Value path has been inspected.

The trace is intentionally not exhaustive yet. In particular, it does not enumerate every value
field nested in pending host/node requests and all module-generation records. Those are the next
targeted roots to inspect before drawing conclusions about the retained Context's external owner.
