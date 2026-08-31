/*---
description: parse-negative expectations are checked before execution
flags: [raw]
negative:
  phase: parse
  type: SyntaxError
---*/
let broken = ;
