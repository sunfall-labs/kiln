// kiln eval smoke test (authored for kiln, not from porffor).
import leven from 'leven';
console.log(leven('kitten', 'sitting'));
console.log(leven('', 'abc'));
console.log(leven('abc', 'abc'));
console.log(leven('gumbo', 'gambol'));
console.log(leven('a', 'b'));
