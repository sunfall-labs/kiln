// kiln eval smoke test (authored for kiln, not from porffor).
const validate = require('validate-npm-package-name');

const good = validate('some-package');
console.log(good.validForNewPackages, good.validForOldPackages);

const scoped = validate('@myorg/my-pkg');
console.log(scoped.validForNewPackages);

const bad = validate(' leading-space');
console.log(bad.validForNewPackages, JSON.stringify(bad.errors));

const core = validate('http');
console.log(core.validForNewPackages, JSON.stringify(core.warnings));

const caps = validate('SomeCaps');
console.log(caps.validForNewPackages, JSON.stringify(caps.warnings));
