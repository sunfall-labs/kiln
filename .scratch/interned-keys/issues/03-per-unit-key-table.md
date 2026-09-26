# 03 — Compile-time key table per translation unit

Status: done
Depends on: 02

## What

In the emitter:

- `property_key_constants(program)` collects the constant indices used by `GetProperty`,
  `SetProperty`, `SetPropertyStrict`, `CallMethod`, `DeleteProperty`, `DeletePropertyStrict` and
  `LoadSuperProperty` across the entry body and every function.
- Emit `static const char *{symbol}_key_{n};` for each, `static int {symbol}_keys_ready;`, and
  `static void {symbol}_intern_keys(void)` that fills them with `kiln_c99_intern_static`.
- `{symbol}_body` (both the scalar and the module renderer) calls `{symbol}_intern_keys` when the
  ready flag is clear, right after registering roots. `{symbol}_gc_roots` clears the flag on
  `KILN_GC_PHASE_RESET`, which the runtime calls at realm destroy for every registered unit, so a
  second realm in the same process re-registers against its own table.
- The listed instructions pass `{symbol}_key_{n}` where they passed `{symbol}_constant_{n}`.
- `kiln_c99_intern_static` returns the bytes themselves if the table cannot grow, so a site never
  holds `NULL`; the read paths then report the key absent, which is consistent because the write
  that would have stored it fails with `KILN_C99_OUT_OF_MEMORY`.

Runtime modules (`render_runtime_module`) carry a hand-written body and no constant table; they
are unchanged.

## Done when

- Snapshots under `crates/native/kiln-backend-c/tests/snapshots` regenerated and the diff reviewed:
  expected changes are the key declarations, the body prologue, the roots reset line, and the
  four instruction forms.
- `emission_is_deterministic_within_a_process` and `equal_programs_emit_equal_units` still pass
  (the table is derived from the program, not from any address).

## Comments
