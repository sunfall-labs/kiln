# PRD: Pin Interned Shape Keys and GC Marking for Active Shapes

Status: resolved
Branch: `fix/async-frame-gc-roots`

## Problem Statement

Values held across `for await` loops in async generator/function frames suffered GC collection or key lookup corruption when dynamic interned keys (such as `"@@iterated"`) were purged from `heap->keys` during garbage collection.

## Root Cause

`kiln_shape` transition trees persist across GC cycles via `kiln_c99_root_shape`. However, dynamically interned keys were allocated as unpinned GC string blocks. During GC marking, `KILN_GC_OBJECT` marked object slots but failed to mark `object->shape->keys`. As a result, weak keys were purged, causing subsequent property lookups to answer `undefined` and iterators to fail with `an array iterator whose source is not an array`.

## Resolution

- Implemented `kiln_c99_pin_interned_key` to pin shape keys in `heap->keys` on shape transition.
- Updated `KILN_GC_OBJECT` marking in `kiln_c99_gc_mark_block` to mark `object->shape->keys[index]`.
- Added conformance probe `conformance/native/array-iterator-gc.js`.
