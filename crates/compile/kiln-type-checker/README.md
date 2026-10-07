# Source checker

`kiln-type-checker` owns source binding, inference, narrowing, type relations and TypeScript diagnostics. Its algorithms are ported from Bun's `src/sema` at `bbdc5a519e0a06d1b3133b564f91096b9ba10a31`, including the Microsoft TypeScript checker/binder ports. `NOTICE` records provenance; `LICENSE-APACHE-2.0` covers the Microsoft portions and declaration libraries.

The public input is an immutable `SourceSnapshot` of owned Kiln modules, auxiliary package metadata, roots and checker options. Oxc parses once and captures original type syntax and original executable syntax before transformations. The private adapter builds the checker representation; it does not parse source again. No donor AST, arena lifetime, symbol ID or type ID crosses the crate API.

The checker returns owned type descriptions, coarse contract classes, diagnostics and a completeness flag. An adapter gap or exhausted stack budget makes a result incomplete. Incomplete results cannot supply bytecode specialization hints. Source contracts, including TypeScript `any` and unchecked assertions, are never Value Graph proofs.

The database receives source checking through a function pointer at the composition root, preserving ADR 0001 and the layer boundaries of ADR 0066. Its memo reads the active source set and parsed module inputs. Bytecode metadata carries the checker identity and its own format version. Numeric aliases therefore reach the existing guarded specialization candidates without creating another executable lowering path (ADR 0061).

The engine interns types and uses dense query tables, task-local buffers, publication barriers and per-session mimalloc arenas. The initial snapshot host schedules serial work and preserves the donor's task publication protocol. A caller with an insufficient stack runs on a scoped worker with an explicit stack allowance. ECMAScript declaration libraries are pinned TypeScript 7.0.2 inputs, parsed once by the composition root; they are not ambient filesystem dependencies.

The Rust toolchain is pinned through mise to `nightly-2026-09-15` for `allocator_api`. Run `mbx` commands under that toolchain. Run the repository gate once after a completed change, retaining its full evidence.

Current integration limits: the snapshot host runs checker tasks serially, and source-set changes conservatively invalidate TypeScript facts. The public options cover strictness and ECMAScript libraries; this is not yet a complete tsconfig or TypeScript compatibility facade. Unsupported owned-syntax adaptation is reported explicitly and disables optimization hints.
