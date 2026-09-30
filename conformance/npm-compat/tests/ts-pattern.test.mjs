// kiln eval smoke test (authored for kiln, not from porffor).
import {match, P} from 'ts-pattern';
console.log(match(2).with(P.number, n => n * 10).otherwise(() => -1));
console.log(match('hello').with(P.string.startsWith('h'), () => 'starts-h').otherwise(() => 'no'));
console.log(match({type: 'ok', v: 1}).with({type: 'ok'}, ({v}) => 'v=' + v).otherwise(() => 'no'));
console.log(match([1, 2]).with([P.number, P.number], ([a, b]) => a + b).otherwise(() => 0));
console.log(match(null).with(P.nullish, () => 'nullish').otherwise(() => 'no'));
console.log(match('x').with(P.union('a', 'b'), () => 'ab').otherwise(() => 'other'));
