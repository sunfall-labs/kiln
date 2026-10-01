// kiln eval smoke test (authored for kiln, not from porffor).
import cliTruncate from 'cli-truncate';
console.log(JSON.stringify(cliTruncate('hello world', 5)));
console.log(JSON.stringify(cliTruncate('hello world', 20)));
console.log(JSON.stringify(cliTruncate('hello world', 8, {position: 'start'})));
console.log(JSON.stringify(cliTruncate('hello world', 8, {position: 'middle'})));
console.log(JSON.stringify(cliTruncate('hello world', 8, {space: true})));
