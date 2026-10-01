// kiln eval smoke test (authored for kiln, not from porffor).
import wrapAnsi from 'wrap-ansi';
console.log(JSON.stringify(wrapAnsi('hello world foo', 5)));
console.log(JSON.stringify(wrapAnsi('supercalifragilistic', 5)));
console.log(JSON.stringify(wrapAnsi('a b c d', 3, {trim: false})));
console.log(JSON.stringify(wrapAnsi('\u001B[31mhello world\u001B[39m', 5)));
