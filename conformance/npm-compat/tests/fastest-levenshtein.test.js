// kiln eval smoke test (authored for kiln, not from porffor).
const { distance, closest } = require('fastest-levenshtein');
console.log(distance('kitten', 'sitting'));
console.log(distance('abc', 'abc'));
console.log(distance('', 'abc'));
console.log(closest('gambol', ['gumbo', 'gamma', 'gambol']));
console.log(closest('hello', ['hallo', 'hollow', 'bye']));
