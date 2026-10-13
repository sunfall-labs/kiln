// Regression found while constructing a large replacement subject.
// Both engines and Node must preserve [97,55357,120,56832,97,55357,120,56832].
const repeated = ('a\ud83dx\ude00').repeat(2);
console.log(JSON.stringify(Array.from({length:repeated.length}, (_,i) => repeated.charCodeAt(i))));
