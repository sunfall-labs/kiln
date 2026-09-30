// kiln eval smoke test (authored for kiln, not from porffor).
const YAML = require('yaml');
console.log(JSON.stringify(YAML.parse('a: 1\nb:\n  - x\n  - y\n')));
console.log(YAML.stringify({a: 1, b: [1, 2]}).trim().split('\n').join(' | '));
const doc = YAML.parseDocument('a: 1');
console.log(doc.get('a'));
const v = YAML.parse('x: true\ny: null\nz: 3.5');
console.log(v.x, v.y, v.z);
console.log(YAML.parse('k: &a 5\nm: *a').m);
