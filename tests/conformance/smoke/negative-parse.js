/*---
description: Kiln smoke test for an expected parse failure
flags: [raw]
features: [kiln-smoke, parser]
negative:
  phase: parse
  type: SyntaxError
---*/
let broken = ;
