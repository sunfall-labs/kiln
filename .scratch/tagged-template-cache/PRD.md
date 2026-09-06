# PRD: Tagged Template Object Caching Across Evaluations

Status: ready-for-agent
Branch: `feat/tagged-template-cache`
Owner: Antigravity, 2026-09-06

## Problem Statement

ECMA-262 §13.2.8.5 (`GetTemplateObject ( templateLiteral )`) mandates that every evaluation of a given syntactic tagged template literal hands the tag function the identical frozen `TemplateStringsArray` object for the lifetime of the Realm.

Currently in Kiln:
```js
function tag(s) { return s; }
function go() { return tag`k`; }
go() === go()      // true in every other engine; false in kiln
```

`ExpressionKind::TemplateStrings` (`crates/kiln-ir/src/compile.rs`) emits instructions that allocate a fresh cooked array, allocate a raw array, freeze both, and attach `raw` from scratch on *every single evaluation*.

## Impact

1. **Real-world library degradation**:
   `lit-html`, `graphql-tag`, `styled-components`, `emotion`, and SQL template libraries key their internal template caches on the object identity (`===`) of the template array. In Kiln, a fresh object per evaluation turns these caches into a 100% cache-miss rate, causing continuous reparsing, degraded performance, and memory leaks.
2. **Conformance**:
   Costs 14 Test262 rows across `language/expressions/tagged-template/cache-*`:
   - `cache-same-site`
   - `cache-same-site-top-level`
   - `cache-different-functions-same-site`
   - `cache-eval-inner-function`

## Architectural Design

The template object must outlive individual function invocations and be shared by every closure over the syntactic call site. It cannot be function-local.

1. **IR & Program Representation**:
   - `Program` stores a `template_objects: Vec<TemplateSite>` table.
   - Each `TemplateSite` stores:
     - `raw: Vec<String>`
     - `cooked: Vec<Option<String>>`
   - A new instruction `LoadTemplateObject(u32)` loads/materializes the template object for site index `i`.
2. **Interpreter Runtime**:
   - `Interpreter` (or `Realm`) maintains a cache slot per site in the running unit (`Option<Value>`).
   - On first evaluation of `LoadTemplateObject(site_index)`:
     - Constructs the frozen `cooked` Array and `raw` Array.
     - Sets `cooked.raw = raw` and freezes both.
     - Caches the resulting `Value::Array` in the realm/program cache slot.
   - Subsequent evaluations immediately return the cached `Value`.
3. **C Backend Runtime**:
   - Emits a static `kiln_val` slot per template site in the module translation unit.
   - Registers static template slots with `render_gc_roots` so the GC retains them.
   - On evaluation, loads the static slot; if null/uninitialized, calls a runtime helper to construct and cache the frozen template array.
4. **Conformance & Parity**:
   - Both engines implement `LoadTemplateObject` (per ADR 0043).
   - Test262 rows in `cache-*` pass under the test runner.
