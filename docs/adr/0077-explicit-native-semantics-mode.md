# ADR 0077: Explicit `--semantics=native` mode trades JavaScript object identity and reflection for fixed-layout C structs, unboxed storage, and zero-overhead native calls

## Status

Proposed, 2026-10-17. Amends ADR 0016 ("A profile changes performance and never semantics") by separating **semantics** (`--semantics=js|native`) from **compilation profile** (`--profile=native-dev|native-release`), and relaxes ADR 0056's boxed-heap restrictions when `--semantics=native` is explicitly selected.

## Context

Under standard JavaScript semantics (`--semantics=js`, the default), every object carries identity (`===`), an extensible property table, prototype chain lookup, and aliasing across structural type boundaries. Even when `kiln-type-checker` proves a value has type `{ x: number, y: number }`, the default native backend must preserve:

1. **Reference aliasing and excess properties across structural narrowing**: Passing `{ x: number, y: number }` where `{ x: number }` is expected must preserve `===` identity, excess properties (`y`), and shared mutation (`narrow.x = 9` mutates `original.x`).
2. **Dynamic prototype, accessor, and reflection hazards**: Every property read and write must account for prototype getters/setters, `delete`, and shape transitions unless proven absent across the whole program.
3. **`double`-only `_t` typed calls and JavaScript call tax**: `typed_signature` only specializes functions whose parameters are all `double`. Any helper taking a typed record falls back to the boxed `kiln_val` ABI. Even on the `_t` path, call sites track call depth (`KILN_C99_MAX_CALL_DEPTH`), push/pop JavaScript stack trace frames, guard against monkey-patched builtins, test `kiln_c99_typed_failed`, and maintain shadow `kiln_val` boxes.
4. **Coarse source-type metadata funnel**: `kiln-hir::source_types` exports coarse `TypeClass` categories (`Number`, `Boolean`, `String`, `Array`, `Tuple`, `Composite`), and `kiln-ir::source_types` maps `TypeClass::Composite` to `None`. Record field layouts never reach bytecode metadata, the SSA Value Graph, or C emission.

On TypeScript-heavy compiler and systems workloads (such as `tsc-ts`), these constraints keep the native backend far from the performance of equivalent C or Rust code.

## Decision

We introduce an explicit `--semantics=js|native` flag (default `js`) on `kiln build` and `kiln run --native`, orthogonal to `--profile`:

```
kiln build app.ts --semantics=native --profile=native-release
```

### 1. Separation of Semantics and Profile

- `--profile` (`native-dev`, `native-release`, `native-release-size`, `native-profile`) governs optimization effort, compilation speed, and whole-program analysis scope; it never changes language semantics.
- `--semantics=js` preserves full ECMAScript semantics; static source types act only as guarded hints.
- `--semantics=native` selects a defined native language contract in which static TypeScript types govern value representation, record layouts, aliasing across narrowing conversions, and direct C calling conventions.
- The native artifact cache key (`kiln-backend-c/src/identity.rs`) hashes `NativeSemantics` and the module's structural record layout table so `--semantics=js` and `--semantics=native` never share compiled artifacts.

### 2. The `--semantics=native` Contract

| Area | Aggressive native behavior (`--semantics=native`) |
| --- | --- |
| Typed records | Fixed C struct field layouts (`typedef struct { ... } kiln_rec_<id>`); direct C member loads and stores (`r.x`, `r->x`) with no shape guards, inline caches, or prototype lookups |
| Narrower record conversion | Project the destination fields into independent storage (`(kiln_rec_narrow){ .x = orig.x }`); mutations and identity may differ from JavaScript (`narrow.x = 9` leaves `original.x` unchanged) |
| Typed function calls | Specialized `_n` native C signatures with unboxed scalar and `kiln_rec_<id>` struct parameters and returns, omitting JS call-depth checks, `js_frame` stack-trace push/pop, builtin monkey-patch guards, and `kiln_val` shadow boxing |
| Arrays | Homogeneous unboxed storage for supported element types |
| Ownership | Borrow, stack-allocate, and eliminate heap allocations where the native contract permits |
| Dynamic operations | Explicitly box into JavaScript `kiln_val` representations at boundaries (e.g. `console.log`), or reject unsupported crossings |
| Reflection | Restrict operations (`delete`, `Object.defineProperty`, `Object.setPrototypeOf`) that require preserving arbitrary dynamic properties and prototype behavior |

### 3. Structural Layout Propagation Through the Compiler Pipeline

1. **`kiln-type-checker` → `kiln-hir::source_types`**:
   - `CheckResult` interns canonical `RecordLayout` descriptors (`RecordLayoutId`, ordered `RecordField`s with `NativeFieldType`: `Number`, `Int32`, `Boolean`, `String`, `Record(RecordLayoutId)`).
   - `SourceType` records an optional `RecordLayoutId` for expressions (`TypeRole::Expression`), parameters (`TypeRole::Parameter`), returns (`TypeRole::Return`), and local binding declarations (`TypeRole::Annotation`).
2. **`kiln-hir` → `kiln-ir::source_types`**:
   - `SourceTypeAnnotations` attaches the module's `RecordLayout` table alongside per-function parameter layouts, return layout, local-slot layouts, and authored-to-generated expression span layouts.
3. **`kiln-ir` → `kiln-graph` → `kiln-backend-c`**:
   - Under `NativeSemantics::Native`, the Value Graph and C emitter lower typed record allocations (`NewObject`), field reads (`GetProperty`), field writes (`SetProperty`), structural narrowing projections, and direct function calls to C structs (`kiln_rec_<id>`) and direct C calls.

## Consequences

- Programs compiled with `--semantics=native` can keep typed records in C structs and CPU registers, pass and return structs and scalars directly across C function calls, and eliminate per-property tag/shape checks and heap allocations.
- Narrowing a record (`const narrow: { x: number } = original`) creates independent storage under `--semantics=native`; mutating `narrow.x` does not mutate `original.x`.
- `--semantics=js` remains the default for all existing commands and conformance suites (`Test262`, `conformance/native`, `conformance/thirdparty`), preserving 100% ECMAScript compatibility unless `--semantics=native` is explicitly requested.
