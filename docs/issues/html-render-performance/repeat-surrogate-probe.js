// Separate pre-existing issue found while constructing a large replacement subject.
// Node/interpreter: [97,55357,120,56832,97,55357,120,56832].
// Native String.repeat currently substitutes U+FFFD for both isolated surrogates.
const repeated = ('a\ud83dx\ude00').repeat(2);
console.log(JSON.stringify(Array.from({length:repeated.length}, (_,i) => repeated.charCodeAt(i))));
