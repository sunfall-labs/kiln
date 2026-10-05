# Combined shared-Array interpreter validation

Source `4010a5c2` (code integration `f17ec9f2`), pinned Rust 1.97.1, mbx release CLI SHA-256 `b8be79d5c64e824c3f0225024cf7d729f78d4ae68120cc45476cc57ebe616644`. Build succeeded in 6m44s with one Cargo worker under the shared three-CPU budget.

Original pinned direct-TypeScript sources and corrected deadline harness: SchemaAOTCompiler 11/11, SchemaCompilerRegression 22/22, toJsonSchemaMultiDocument 32/32. All 65 registered cases recorded and mapped exactly to the official Node/Vitest oracle, zero failures, skips, gaps, inventory or outcome mismatches. Per-case deadlines unchanged.

The prior regression candidate passed only 21/22 SchemaCompilerRegression cases; this combined candidate passes the full 22-case file. The receipt covers three files, with 319 deferred. It does not establish full-suite acceptance or the final constructor/realm-guard follow-ups. Full raw logs and case-level reports are retained here; no binaries are committed.
