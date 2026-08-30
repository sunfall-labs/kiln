// String methods. Every position here counts UTF-16 code units (ADR 0022), which is why the
// backend converts once and works on units rather than walking bytes per method: a dozen ad-hoc
// walks is a dozen chances to disagree with the interpreter about where a position is.
const s = "Hello, World";
console.log(s.length, s.slice(0, 5), s.slice(7), s.slice(-5), s.slice(3, 1), s.slice(0, 99));
console.log(s.substring(0, 5), s.substring(5, 0), s.substring(-3, 2), s.substring(7));
console.log(s.indexOf("o"), s.indexOf("zz"), s.lastIndexOf("o"), s.includes("World"));
console.log(s.startsWith("Hello"), s.startsWith("World"), s.endsWith("World"), s.endsWith("x"));
console.log(s.charAt(0), s.charAt(99), s.charCodeAt(0), s.charCodeAt(99), s.at(0), s.at(-1), s.at(99));
console.log(s.toUpperCase(), s.toLowerCase(), "".toUpperCase());

console.log("  padded  ".trim(), "  x".trimStart(), "x  ".trimEnd(), "".trim());
console.log("a,b,c".split(","), "abc".split(""), "abc".split("x"), "a".split(","));
console.log("ab".repeat(3), "ab".repeat(0), "".repeat(5));
console.log("5".padStart(3, "0"), "5".padEnd(3, "*"), "abc".padStart(2), "x".padStart(5));
console.log("a-b-a".replace("a", "X"), "a-b-a".replaceAll("a", "X"), "abc".replace("z", "X"));
console.log("ab".concat("cd", "ef"), "".concat("x"), "a".concat());

// The methods are values, and the same one on two strings is the same function.
console.log("a".slice === "b".slice, typeof s.trim, s.slice, s.toUpperCase);

// A method reached through an expression, and chained calls -- which is how they are written.
console.log(("  " + s + "  ").trim().toLowerCase().split(", ").join("|"));
console.log(s.slice(0, s.indexOf(",")).toUpperCase(), s.split(", ").map((w: string) => w.length));

// Slicing to `length` gives the string back, which is the invariant that broke when positions
// meant two different things.
const accented = "café";
console.log(accented.length, accented.slice(0, accented.length) === accented, accented.at(-1));
console.log(accented.indexOf("é"), accented.slice(accented.indexOf("é")), accented.split("é"));
