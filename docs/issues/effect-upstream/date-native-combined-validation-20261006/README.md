# Combined Date and sealed lookup validation

The clean `e1fa4d16` candidate CLI (`824b7a2604c0a9dcf6bd01a6b649c83372bb21a816eda61d8cd3e16fd98df800`) passes all 43 original upstream Cron cases directly from TypeScript with unchanged case deadlines. Inventory, modes and case IDs exactly match the complete main interpreter survey. This candidate excludes the isolated StringUnits comparison change.

Five affected Date native probes now match, both ordinarily and with collection stressed on every allocation. This includes computed `Date.prototype[method].call` BigInt rejection, which diverged in the earlier combined Date candidate. The common own-property lookup now reads sealed callable stores instead of falling through to a raw Date storage method. The captured-intrinsics and UTC clock-setter probes also match across 1, 2 and 16 concurrent realms; stressed runs match across 1 and 2 realms. Per-realm outputs and statuses are retained without executable binaries.

The Date Test262 subtree contains exactly the same 1,188 variants as the frozen main comparison: 1,166 pass and the same 22 fail, with no changed outcomes. This is a subtree check. Whole-program facts match all 31 committed targets (14 benchmarks and 17 bundles).

The candidate also contains the isolated native hash-width and collector lookup-reuse changes; these focused Date checks do not establish their Effect deadline performance. They do not establish a complete native or interpreter Effect pass, a routine gate pass, or quiet benchmark performance. The remaining original deadline failures and the final Bun comparison remain open. Complete logs, source/binary receipts, reports and file digests accompany this record.
