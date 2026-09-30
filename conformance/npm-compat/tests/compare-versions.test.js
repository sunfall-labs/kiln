// kiln eval smoke test (authored for kiln, not from porffor).
const { compareVersions, compare } = require('compare-versions');
console.log(compareVersions('1.2.3', '1.2.4'));
console.log(compareVersions('2.0.0', '1.9.9'));
console.log(compare('1.0.0', '1.0.0', '='));
console.log(compare('1.2.0', '1.10.0', '<'));
console.log(compareVersions('1.0.0-alpha', '1.0.0'));
