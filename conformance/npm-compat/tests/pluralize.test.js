// kiln eval smoke test (authored for kiln, not from porffor).
const pluralize = require('pluralize');

console.log(pluralize('cat'));
console.log(pluralize('box'));
console.log(pluralize('mouse'));
console.log(pluralize('sheep'));
console.log(pluralize('cat', 2, true));
console.log(pluralize.singular('children'));
console.log(pluralize.addPluralRule(/x$/i, 'xen') && pluralize('boxx'));
