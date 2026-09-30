// kiln eval smoke test (authored for kiln, not from porffor).
import sliceAnsi from 'slice-ansi';
console.log(JSON.stringify(sliceAnsi('\u001B[31mhello\u001B[39m', 1, 3)));
console.log(JSON.stringify(sliceAnsi('hello', 1, 3)));
console.log(JSON.stringify(sliceAnsi('古a', 0, 1)));
console.log(JSON.stringify(sliceAnsi('hello', 0, 5)));
