# Interpreter callsite names across nested calls

Status: complete

The interpreter must preserve a suspended caller's method name while a nested call runs. Restoring a frame now restores `method_name` with the other activation state on normal return and both exception-unwind paths. Stack-name selection ignores empty names so an anonymous function can fall back to its callsite alias.

A conformance regression covers an anonymous computed method whose body performs a nested call that returns, and one whose nested call throws and is caught. Both require the callsite alias to remain in the captured stack.

Validation used the pinned Effect source commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d` and release interpreter CLI SHA `b800b2c340daf2ae1d4dc981c8628bd69d31452d96ef6aeb6dfd4c9e9afe0a02`:

- Cause upstream: 127 passed, 0 failed, 0 skipped.
- `kiln-interpreter` unit tests: 243 passed, 0 failed, 1 ignored.
- Focused `regress/r11-method-name-survives-nested-calls`: 1 passed.
- `cargo fmt --all` and `git diff --check` passed.

The complete per-file Cause report and raw case log are retained beside this note.
