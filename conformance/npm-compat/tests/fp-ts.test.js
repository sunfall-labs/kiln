// kiln eval smoke test (authored for kiln, not from porffor).
const O = require('fp-ts/lib/Option');
const E = require('fp-ts/lib/Either');
const { pipe } = require('fp-ts/lib/function');
console.log(JSON.stringify(pipe(O.some(2), O.map(x => x * 3))));
console.log(O.isSome(O.none), O.getOrElse(() => 0)(O.none));
console.log(JSON.stringify(pipe(E.right(5), E.map(x => x + 1), E.getOrElse(() => -1))));
console.log(JSON.stringify(pipe(E.left('err'), E.map(x => x), E.fold(e => 'L:' + e, v => 'R:' + v))));
console.log(pipe(O.some(1), O.chain(x => (x > 0 ? O.some(x * 10) : O.none)), O.toUndefined));
