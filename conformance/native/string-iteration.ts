// A string is iterable, and there are two entirely different ways a program reaches that.
//
// Spread hands the value to one runtime walk; `for...of`, array destructuring and `yield*` are all
// compiled as `value[Symbol.iterator]()` and reach the *method* instead. Nothing in this corpus
// asked the second question of a string, so the method answered arrays, typed arrays and
// array-likes and refused everything else -- and `[...text]` worked while `for (const c of text)`
// stopped the program. A bundled `marked` refused there, tokenising a list item.
//
// Both spellings are asked of the same strings here, so the two can never again disagree about
// what a string yields.

const plain = "a\tb";

// The shape that refused. Each step is a *character*, and the loop variable is a one-character
// string rather than a code unit number.
const stepped: string[] = [];
for (const character of plain) stepped.push(character);
console.log(stepped.join("|"), stepped.length);

// The shape that worked, over the same string: the two must agree element for element.
console.log([...plain].join("|"), [...plain].length === stepped.length);

// Astral characters are one step, not two, which is the whole difference between iterating a
// string and indexing it: this string is three characters and four code units long. Code points
// rather than the characters themselves, so what is compared is the walk and not two engines'
// opinions about printing non-ASCII.
const astral = "a\u{1F600}b";
const points: number[] = [];
for (const character of astral) points.push(character.codePointAt(0) as number);
console.log(points.join(","), points.length, astral.length);

// A lone surrogate has no pair to join, and iteration must not invent one.
const lone = "x\uD83Dy";
console.log([...lone].length, lone.length);

// Destructuring reads what the value *yields*, so it goes through the same method.
const [first, second] = astral;
console.log(first, second.length, second.codePointAt(0));

// `yield*` delegates to the same protocol: one more caller of the same method.
function* letters(): Generator<string> {
  yield* plain;
}
console.log([...letters()].join("|"));

// The method reached by name, which is what the three shapes above compile to.
const walk = plain[Symbol.iterator]();
const step = walk.next();
console.log(step.value, step.done, walk.next().value);

// An empty string yields nothing and completes, rather than refusing or looping.
let empties = 0;
for (const character of "") empties += character.length + 1;
console.log("empty steps:", empties);

// `Array.from` and the collection constructors take the same walk.
console.log(Array.from(astral).length, new Set(plain).size, new Set("aab").size);
