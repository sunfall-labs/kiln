# Unreachable owner cycles retain WeakRef targets

A resource reachable only through an unreachable ordinary object self-cycle,
an array self-cycle, or a mixed object/array cycle survives eight GC jobs in
kiln. Node collects all three. A resource without a cycle is collected by both;
a resource with a live owner remains reachable in both. The same discrepancy
is present in the pushed production binary and the integrated TypeScript
candidate. The probe deliberately finishes its initial module job before
collection, preserving the existing WeakRef job-root contract.

Expected first line: `true,true,true,true`. Both kiln runs print
`false,false,false,true`. All runs print `true` for the live-owner control.
The attached receipt gives source and executable identities and file digests.
These are diagnostic outputs, not passing upstream Pool tests.

This proves that the collector misses unreachable incoming owner cycles when
only the resource is a weak-census seed. It does not prove which holder retains
Effect Pool contexts; the full owner-path diagnosis and live-preservation checks
remain necessary before choosing a production collector change.
