// kiln eval smoke test (authored for kiln, not from porffor).
const semver = require('semver');
console.log(semver.valid('1.2.3'));
console.log(semver.valid('not-a-version'));
console.log(semver.satisfies('1.2.3', '^1.0.0'));
console.log(semver.satisfies('2.0.0', '^1.0.0'));
console.log(semver.gt('2.0.0', '1.9.9'));
console.log(semver.inc('1.2.3', 'major'));
console.log(semver.coerce('v3.4'));
console.log(semver.major('3.4.5'));
