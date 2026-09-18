# PRD: Side-Effect-Free Top-Level Declaration Pruning in Closed-World Graphs

Status: complete
Feature: removable-declarations

## Problem Statement
In closed-world builds (such as native compilation of bundles and applications like ghui or Node packages), Kiln tracks `RemovableDeclaration` to omit un-demanded module-level declarations and their associated functions.

However, `removable_declaration_names` in `crates/kiln-ir/src/compile.rs` previously recognized only:
- Declarations with no initializer (`let x;`)
- Functions (`const f = function() {}`)
- Classes without heritage (`class C {}`)

It completely omitted:
- Literal values: numbers, strings, booleans, null, undefined, bigint (`const A = 1; const S = "hi";`)
- Pure unary expressions on literals (`const NEG = -1; const VOID = void 0;`)
- Array literals composed of pure expressions (`const LIST = [1, 2, 3];`)
- Object literals with static keys and pure values (`const CONFIG = { x: 1, y: 2 };`)

As a result, in large libraries (e.g. `lodash`, `core-js`, `ajv`, `dayjs`, `postcss`, OpenTUI bundles) where hundreds of static configuration tables, error codes, and constants exist at module scope, none could be pruned when un-demanded. Their bytecode instructions remained live, their module slots were allocated, their constants were referenced, and any dead helper closures they touched could not be pruned.

## Goal & Architecture
1. **Purity Evaluation**:
   Introduce `is_removable_expression(expr: &Expression) -> bool` in `crates/kiln-ir/src/compile.rs` to recognize side-effect-free initializers:
   - `ExpressionKind::Literal(_)`
   - `ExpressionKind::Function(_)`
   - `ExpressionKind::Class(class)` if `removable_class_evaluation(class)`
   - `ExpressionKind::Unary` with pure operators (`+`, `-`, `!`, `void`) on pure arguments
   - `ExpressionKind::Array` with pure elements
   - `ExpressionKind::Object` with non-`__proto__` static keys and pure values
2. **Bytecode Pruning**:
   When an un-demanded declaration is marked removable and none of its local slots are in `module_live`, `closed_world_function_liveness` omits its bytecode (turning it into `Nop`), leaves its slots dead, and avoids marking any referenced functions as live.
3. **Verification**:
   - Unit tests verifying removable declaration detection and dead constant elimination.
   - 100% pass on all existing tests (`kiln-ir`, `kiln-backend-c`, `kiln-native`, `emitted_c`).
   - Parity suite (`scripts/native.sh`) and thirdparty bundles (`scripts/thirdparty.sh`).
   - Quick gate (`mise run gate-quick`).
