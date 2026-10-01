// kiln eval smoke test (authored for kiln, not from porffor).
import ansiRegex from 'ansi-regex';
const re = ansiRegex();
console.log(re.test('\u001B[31mred\u001B[39m'));
console.log(re.test('plain'));
console.log(JSON.stringify('a\u001B[1mb\u001B[22mc'.replace(re, '')));
const onlyFirst = ansiRegex({onlyFirst: true});
console.log(JSON.stringify('x\u001B[31my\u001B[39mz'.replace(onlyFirst, '')));
