// kiln eval smoke test (authored for kiln, not from porffor).
const stringSimilarity = require('string-similarity');

console.log(stringSimilarity.compareTwoStrings('healed', 'sealed'));
console.log(stringSimilarity.compareTwoStrings('abc', 'xyz'));
console.log(stringSimilarity.compareTwoStrings('', ''));
console.log(
  JSON.stringify(
    stringSimilarity.findBestMatch('hello', ['hallo', 'world', 'hell']).bestMatch
  )
);
console.log(stringSimilarity.compareTwoStrings('case', 'CASE'));
