# 01. Mark kiln extensions and add the switch that removes them

Status: ready-for-agent

ADR 0068, rules 1, 2, 4, 5 and 6.

1. Add a "kiln extension" mark to the built-in member description (`kiln-builtin-surface`), and
   mark `parallelMap` on `Array.prototype` and `%TypedArray%.prototype`.
2. Both engines skip installing marked members when `KILN_EXTENSIONS=0`; `kiln build
   --no-kiln-extensions` records the same in a binary. With either, the built-ins are exactly
   Node's.
3. The surface inventory reports marked members in their own column, not as "extra (node lacks
   it)".
4. Check every marked member is non-enumerable, as a test over the description.
5. `packages/kiln-concurrency/index.js` already defines its fallback only when the method is
   absent; keep that, and add a test that under kiln the engine's method is the one in place.

## Acceptance

- `KILN_EXTENSIONS=0 kiln run` and a `--no-kiln-extensions` binary: `'parallelMap' in []` is
  `false` and `Object.getOwnPropertyNames(Array.prototype)` matches Node's, in both engines.
- Test262, `scripts/thirdparty.sh` and (once it exists) the deploy corpus pass identically with
  extensions on and off.
