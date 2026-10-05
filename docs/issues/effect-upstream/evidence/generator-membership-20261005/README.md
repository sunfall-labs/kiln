# Generator inherited membership

The interpreter's `HasProperty` skipped links between generator prototypes and their iterator
prototypes. Native membership used an intrinsic method list even when the TypeScript-owned
prototype was authoritative, incorrectly finding synchronous iteration on async generators and
missing properties added to generator prototypes. Both now traverse the actual prototype chain
without executing accessors.

Source baseline: `4b5e6c9cc68661f19599ec02ed665e9a47379270`.
Candidate CLI SHA-256: `42d996860b9df38c600e7a4b02b5625d6d50d5b3d0ae607fe42bc9f6bbe25e99`.
Effect source pin: `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.

Validation:

- Interpreter crate: 258 passed, zero failed, one ignored.
- Original direct-TS Effect Result entry: 44 passed, zero failed or skipped (previously 43/44).
- Native generator membership probe: one matched, zero refused or diverged; includes async
  iteration, inherited accessor membership, and `Array.from`.
- Pinned Test262 `language/statements/generators`: all 510 case IDs have identical baseline and
  candidate outcomes (502 passed, eight existing failures). This is subtree coverage.
- Formatting and diff checks passed.

Compressed case streams and both Test262 result documents are preserved beside this receipt.
The full Effect suite and routine gate have not been rerun on this change.
