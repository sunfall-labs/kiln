// kiln eval smoke test (authored for kiln, not from porffor).
import stripAnsi from 'strip-ansi';

console.log(JSON.stringify(stripAnsi('\u001B[31mred\u001B[39m')));
console.log(stripAnsi('plain'));
console.log(JSON.stringify(stripAnsi('\u001B[1;32mbold green\u001B[0m end')));
console.log(stripAnsi('\u001B[4munderline\u001B[24m'));
