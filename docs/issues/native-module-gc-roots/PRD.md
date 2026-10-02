# PRD: Module-Scope Globals GC Roots in Native C Compilation

Status: resolved
Branch: `fix/native-module-gc-roots`

## Problem Statement

Compiling an ES module with module-scope global declarations via `kiln run --native` failed with C compilation errors:
`error: use of undeclared identifier 'KILN_GC_PHASE_MARK'`
`error: call to undeclared function 'kiln_c99_gc_mark_values'`

## Resolution

Added missing declarations in `prelude_declarations.h`:
- Defined `KILN_GC_PHASE_MARK`.
- Declared opaque `struct kiln_gc_heap`.
- Added `extern void kiln_c99_gc_mark_values(...)`.
