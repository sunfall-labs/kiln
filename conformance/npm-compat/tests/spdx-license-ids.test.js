// kiln eval smoke test (authored for kiln, not from porffor).
const ids = require('spdx-license-ids');

console.log(Array.isArray(ids));
console.log(ids.includes('MIT'));
console.log(ids.includes('Apache-2.0'));
console.log(ids.includes('Not-A-Real-License-9.9'));
console.log(ids.length > 400);
