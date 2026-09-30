// kiln eval smoke test (authored for kiln, not from porffor).
const yaml = require('js-yaml');
console.log(yaml.dump({ a: 1, b: [1, 2], c: 'str' }));
console.log(JSON.stringify(yaml.load('a: 1\nb:\n  - x\n  - y\n')));
console.log(yaml.dump({ nested: { deep: true } }, { indent: 4 }));
const doc = yaml.load('---\nfoo: bar\nnum: 42\nflag: true\n');
console.log(doc.foo, doc.num, doc.flag);
console.log(JSON.stringify(yaml.load('[1, 2, 3]')));
