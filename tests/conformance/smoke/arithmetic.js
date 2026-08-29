/*---
description: Kiln smoke test for scalar execution and host output
flags: [raw]
features: [kiln-smoke, arithmetic, console-log]
---*/
let total = 1;
while (total < 4) total += 1;
console.log("smoke", total);
