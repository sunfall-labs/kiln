# 10. Ownership transfer

Status: ready-for-agent
Blocked by: 05; 08 for the read path it shares

ADR 0070's primitive C: move a group of blocks from one realm's heap to another's by relinking them,
without copying.

1. **Transfer lists first**: `postMessage(buffer, [buffer])` moves an `ArrayBuffer`'s backing store
   and detaches it in the sender, exactly as Node does. This makes 06's transfer lists zero-copy.
2. **ADR 0069's worker arenas**, which join the realm's heap at the end of a parallel call
   (`.scratch/parallel-builtins/` issue 08 builds on this).
3. **Object graphs** between realms, once a proof exists that the sender keeps no reference into
   the graph. That proof replaces the name-matched `isolate` escape check in `kiln-hir`, which
   should be removed or restricted to the built-in API when 12 decides it.

## Acceptance

- Transferring a 100 MB `ArrayBuffer` between realms costs the same as transferring 1 KB.
- `mise run native-stress` clean across transfer probes.
