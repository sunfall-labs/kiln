# Static TypeScript Formatter validation

This evidence belongs to the isolated candidate worktree. It does not claim a
full 322-file Effect run or a landing gate.

The final release CLI used for the semantic acceptance runs is SHA-256
`15446ec78485674537cee1b654c0566e7101b3813ad2355750af9c5c1437d860`.
It was built from HEAD `2b4e31c0acee20f380ba8d93b398e4d976ec618a` plus the
candidate diff. Before the Formatter acceptance runs, only a Rust doc comment
and this PRD's status text changed; neither changes compiled behavior. The
source module hash is `9d816f3751b14da252a9fa3c5082668a08d8d2b683bd4878ae5da3d87f503399`.

Pinned input: Effect commit `67ba4e46a11ccda0b6761578bfd22c04ae00167`;
Formatter selected as one complete file, 67 registered cases. The independent
Node/Vitest oracle is Node 26.8.1 / Vitest 5.0.3. Both runs kept original case
budgets; the runner outer timeout was 2400 seconds.

| Engine | CLI profile | Result | Report SHA-256 |
| --- | --- | --- | --- |
| Interpreter | Release CLI | 67 passed, 0 failed/skipped/timeouts | `1b66094039be4f2b6703e72f564c8e35a19cb929b94ec967128fbe44ab5fe7ac` |
| Native | Release CLI and native-release output | 67 passed, 0 failed/skipped/timeouts | `f48d5f3986358874a1b5454fe3053126c3bd42b0be026e02f18b21f0042d728f` |

Both reports record all 67 official cases as mapped, with zero mapping,
outcome, mode, inventory, or known source-transform-gap mismatches. The native
provider manifest SHA-256 is
`b03c48a204de3f911b424ce62b7d96a88ef595de0f99217cba8c34caf6c5dd6e` (ABI 82).
The reports are retained at
`.scratch/final-candidate-formatter/formatter-interpreter-release/report.json`
and
`.scratch/final-candidate-formatter/formatter-native-release/report.json`.

The four focused TypeScript native parity probes also matched: reference erasure,
enums, optional-chain DCE, and primitive-delete DCE. Captured output is in
`.scratch/final-candidate-formatter/native-typescript-probes.log`. The focused
native module-graph test
`tests::module_graph::native_import_meta_reparse_reuses_static_typescript_source_map`
passes and executes generated TS with DCE, checks that an unused import is
removed, preserves the original TS call-site line, and verifies `import.meta.url`
still names `provider.ts`. The backend test
`tests::emission::program_hash_covers_static_transform_identity` verifies that
identical path-insensitive TypeScript modules at distinct absolute roots share
static transform identity and native program hash. Path-sensitive `import.meta`
is tested separately and is not expected to reuse an identical artifact.

The release CLI candidate is the acceptance binary. A previous debug-CLI native
Formatter run also passed 67/67, but its long graph-build time makes it diagnostic
only and it is not used for performance claims.
