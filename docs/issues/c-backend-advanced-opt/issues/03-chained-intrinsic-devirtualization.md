# Issue 03: Chained Intrinsic Call Devirtualization

Status: closed
Feature: c-backend-advanced-opt

## Objectives
1. In `call_method.rs`:
   - When receiver `object` is proven to be an Array (e.g. from `NewArray` or prior `map`/`filter`/`slice`), simplify the tag test `(v{o} & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX && kiln_c99_gc_kind_of_ptr(ptr) == KILN_GC_ARRAY` to avoid redundant checks in chained pipelines.
2. Verify with test cases for chained array operations like `[1, 2, 3].map(x => x * 2).filter(x => x > 2)`.
