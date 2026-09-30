// kiln eval smoke test (authored for kiln, not from porffor).
const { of, from, map, filter, reduce, toArray } = require('rxjs');

const out1 = [];
of(1, 2, 3).subscribe((v) => out1.push(v));
console.log(JSON.stringify(out1));

const out2 = [];
of(1, 2, 3, 4)
  .pipe(
    filter((x) => x % 2 === 0),
    map((x) => x * 10)
  )
  .subscribe((v) => out2.push(v));
console.log(JSON.stringify(out2));

from(['a', 'b'])
  .pipe(
    map((s) => s.toUpperCase()),
    toArray()
  )
  .subscribe((arr) => console.log(JSON.stringify(arr)));

of(1, 2, 3)
  .pipe(reduce((acc, x) => acc + x, 0))
  .subscribe((sum) => console.log(sum));
