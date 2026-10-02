# Conformance and Test262 Verification for Tagged Template Caching

Status: resolved

## Comments

Resolved and verified:
- All 14 Test262 tests in `language/expressions/tagged-template/cache-*` pass.
- Conformance differential passes (109 matched, 0 diverged) including updated `tagged-templates.js`.
- Conformance native parity passes (118 matched, 0 diverged) including new `tagged-template-cache.js`.

## Goal

Verify that 14 Test262 tests in `language/expressions/tagged-template/cache-*` pass, and verify differential and native parity.

## Details

1. Test262 suite:
   - Run Test262 tagged-template tests under `language/expressions/tagged-template/cache-*`:
     - `cache-same-site.js`
     - `cache-same-site-top-level.js`
     - `cache-different-functions-same-site.js`
     - `cache-eval-inner-function.js`
2. Differential tests:
   - Add a differential probe in `conformance/differential/` checking:
     ```js
     function tag(strings) { return strings; }
     function evaluate() { return tag`hello`; }
     console.log(evaluate() === evaluate()); // must be true
     ```
3. Native parity:
   - Add `conformance/native/tagged-template-cache.ts`.
   - Run `./scripts/native.sh`.
