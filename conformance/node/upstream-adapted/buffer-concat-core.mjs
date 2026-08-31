const zero = Buffer.concat([]);
const oneSource = Buffer.from("asdf");
const one = Buffer.concat([oneSource]);
const long = Buffer.concat(Array.from({ length: 10 }, () => Buffer.from("asdf")));
const limited = Buffer.concat([Buffer.from("abcdef")], 4);
const padded = Buffer.concat([Buffer.from("ab")], 4);
const typed = Buffer.concat([new Uint8Array([65, 66]), new Uint8Array([67, 68])]);

if (zero.length !== 0) throw new Error("empty concat");
if (one.toString() !== "asdf" || one === oneSource) throw new Error("single concat copy");
if (long.toString("utf8") !== "asdf".repeat(10)) throw new Error("long concat");
if (limited.toString() !== "abcd") throw new Error("explicit truncation");
if (padded.length !== 4 || padded[2] !== 0 || padded[3] !== 0) throw new Error("zero-filled tail");
if (typed.toString() !== "ABCD") throw new Error("Uint8Array inputs");

console.log("pass");
