# PRD: Differential Conformance Parity & Refusal Classification

Status: resolved
Branch: `main`

## Problem Statement

1. `scripts/differential.sh` reported 5 divergences:
   - `proxy-descriptor-keys`: `Array` static ownKeys enumeration differed (`fromAsync|of` in Node v26 vs `of|fromAsync` in `kiln_builtin_surface::SURFACE`).
   - `accessor-properties`, `delete-properties`, `frozen`, `inherited-setters`: Failed with non-zero exit codes in Kiln due to strict-mode module execution (`d72dac66`). However, because `d959904e` formatted multiline diagnostics without the `kiln:` prefix, `scripts/differential.sh` failed to recognize them as refusals and misreported them as silent wrong-answer divergences.

## Resolution

1. Updated `crates/kiln-builtin-surface/src/lib.rs` to order `Array` statics as `["isArray", "from", "fromAsync", "of", "@@species"]`, matching ECMAScript §23.1.2 and Node v26.
2. Updated `scripts/differential.sh` to properly classify non-zero exit statuses containing `could not execute module` or multiline diagnostics as refusals.
3. Verified:
   - `./scripts/differential.sh`: 114 matched, 4 refused, 0 diverged.
   - `tests/builtin_surface_drift.rs`: 3 passed, 0 failed.
