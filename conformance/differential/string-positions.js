// Every position-taking string method counts UTF-16 code units, which is what `length` counts.
// Getting this wrong is silent: `slice` counting characters agreed with node for all-ASCII text
// and disagreed for anything else, and no probe here had ever sliced a string with an emoji in
// it. See ADR 0022 -- kiln cannot store an unpaired surrogate, so the cases that produce one are
// deliberately not printed below; everything that survives the round trip is.
const plain = "abcdef";
const astral = "a\u{1F600}b";
const accented = "café";

console.log(plain.length, astral.length, accented.length, [...astral].length);

// The invariant a program depends on: slicing to `length` gives the string back, and the three
// spellings of "the character there" name the same position.
console.log(plain.slice(0, plain.length) === plain, astral.slice(0, astral.length) === astral);
console.log(accented.slice(0, accented.length) === accented, astral.slice(0) === astral);
console.log(astral[0] === astral.charAt(0), astral.at(0) === astral.charAt(0));
console.log(astral.at(-1), astral.charAt(3), astral[3], astral.charCodeAt(0));

// A cut on a pair boundary keeps the pair whole on both sides.
console.log(astral.slice(1, 3), astral.slice(3), astral.slice(0, 1), astral.substring(1, 3));

// An index is reported in the same units, so it can be fed straight back to `slice`.
console.log(astral.indexOf("b"), astral.lastIndexOf("b"), astral.slice(astral.indexOf("b")));
console.log(accented.indexOf("é"), accented.slice(accented.indexOf("é")));
console.log(plain.indexOf("cd"), plain.lastIndexOf("c"), plain.slice(plain.indexOf("cd"), 4));

// Negative and out-of-range positions, where the four spellings deliberately differ.
console.log(plain.slice(-2), plain.slice(2, 1), plain.substring(4, 2), plain.at(-1));
console.log(JSON.stringify(plain.charAt(99)), plain[99], plain.at(99), plain.slice(99));
