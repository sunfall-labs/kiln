# Issue 02: Dense Static Key Table Compaction

Status: closed
Feature: c-backend-advanced-opt

## Objectives
1. In `crates/kiln-backend-c/src/lib.rs` (`render_key_table`):
   - When `symbol == "ku"`, emit `static const kiln_string *const ku_static_keys[] = { &ku_string_0, ... };` and an intern loop:
     ```c
     for (size_t i = 0; i < sizeof(ku_static_keys)/sizeof(ku_static_keys[0]); ++i) {
         if ((ku_keys[i] = kiln_c99_intern_static_key(ku_static_keys[i])) == NULL) return KILN_C99_OUT_OF_MEMORY;
     }
     ```
   - Avoid unrolling hundreds or thousands of separate `if ((ku_keys[i] = ...) == NULL)` lines in compact modules.
2. Verify all unit tests and snapshots.
