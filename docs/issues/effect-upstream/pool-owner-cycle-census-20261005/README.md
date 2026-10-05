# Pool owner-cycle acceptance

The final weak-allocation-census source passes all 47 pinned upstream Pool tests in the interpreter, with all 47 mapped to the official Node/Vitest oracle and zero mapping, mode, or outcome mismatches. Original per-case budgets are unchanged. The runner outer safety deadline is 600 seconds. This is focused acceptance, not a complete 322-file run.

The interpreter crate passes 258 tests, with one ignored. Added checks cover disconnected object, array and mixed cycles; live external owners; another realm’s live cycle; weak-only census entries; and bounded pruning of dead allocation records. Every production ObjectData allocation now uses tracked_object, including the iterative JSON parser. ArrayData::new centrally registers arrays. Existing collector root and ephemeron accounting is unchanged.

The census contains thread-local weak identities only. Rc values cannot cross threads. During collection each identity is upgraded once into the existing deduplicated graph. Another realm’s live references remain external strong owners. Candidate snapshots contain weak references, and expired entries are pruned after temporary collector owners are dropped. No Effect-specific ownership rules or Scope mutations are introduced.

The compressed report, per-case output, full crate-test/build logs, and runner log are preserved here. receipt.json records source and executable identities plus evidence digests.
