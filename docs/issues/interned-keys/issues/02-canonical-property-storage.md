# 02 — Every stored key is canonical; hot scans compare pointers

Status: done
Depends on: 01

## What

- `kiln_c99_own_slot` interns the key before storing it (the only ordinary write site).
- The namespace wrapper interns each export name (the other write site).
- `kiln_c99_new_symbol` and the registry-symbol path register their reserved `@@symbol:` key with
  `kiln_c99_intern_static` (the bytes are malloc-backed and never released, as the comment there
  already says), so a symbol-keyed property has pointer identity without a copy.
- Convert the scans in `kiln_c99_get_own_kind` (object chain), `kiln_c99_put_mode`,
  `kiln_c99_own_slot`, `kiln_c99_has_property`, `kiln_c99_delete`, `kiln_c99_own_private` and
  `kiln_c99_write_would_accept` to: canonicalize the query once (`interned` for reads, `intern`
  for writes), then compare `property->key == ikey`. A read whose key is not interned skips the
  scan entirely.
- `kiln_c99_same_key` stays for the unconverted cold paths (it is still correct: bytes-equal is
  key-equal, and canonical storage makes its pointer short-cut hit more often).
- `kiln_c99_is_node_hidden_key` checks the `@@` prefix before anything else.
- `KILN_KEYS_CHECK=1`: converted scans also run the byte comparison and `abort()` with a message
  naming the key if a pointer miss was a byte hit. Read once at heap init like `KILN_GC_STRESS`.

## Done when

- `cargo test -p kiln-backend-c -p kiln-native` is at baseline or better.
- `KILN_KEYS_CHECK=1 KILN_GC_STRESS=1 scripts/native.sh` matches the plain run.

## Comments
