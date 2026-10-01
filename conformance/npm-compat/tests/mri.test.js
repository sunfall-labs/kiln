// kiln eval smoke test (authored for kiln, not from porffor).
const mri = require('mri');

const r1 = mri(['--foo', 'bar', '-n', '42', '--no-cache', 'pos1', 'pos2']);
console.log('parsed:', JSON.stringify(r1));

const r2 = mri(['--count=5', '--verbose'], {
  boolean: ['verbose'],
  default: { count: 0, verbose: false }
});
console.log('typed:', r2.count, typeof r2.count, r2.verbose);

const r3 = mri(['-abc'], { boolean: ['a', 'b', 'c'] });
console.log('short flags:', r3.a, r3.b, r3.c);

const r4 = mri(['--list', 'a', '--list', 'b'], { array: ['list'] });
console.log('array:', JSON.stringify(r4.list));

const r5 = mri(['--', '--not-a-flag'], {});
console.log('after --:', JSON.stringify(r5._));
