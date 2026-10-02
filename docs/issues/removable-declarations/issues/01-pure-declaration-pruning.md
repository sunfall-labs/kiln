# Issue 01: Side-Effect-Free Top-Level Declaration Pruning

Status: complete
Feature: removable-declarations

## Objectives
1. In `crates/compile/kiln-ir/src/compile.rs`:
   - Implement `is_removable_expression(expr: &Expression) -> bool` handling literals, pure unaries, pure arrays, pure objects, functions, and pure classes.
   - Update `removable_declaration_names` to use `is_removable_expression`.
2. In `crates/compile/kiln-ir/src/tests.rs` or `crates/native/kiln-backend-c/src/tests.rs`:
   - Add unit test verifying that unread top-level constants, arrays, and objects are recorded as `RemovableDeclaration` and pruned in closed-world liveness.
3. Verify:
   - `mbx test -p kiln-ir`
   - `mbx test -p kiln-backend-c --lib`
   - `mbx test -p kiln-backend-c --test emitted_c`
   - `mbx test -p kiln-native`
   - `./scripts/native.sh`
   - `./scripts/thirdparty.sh`
   - `mise run gate-quick`
