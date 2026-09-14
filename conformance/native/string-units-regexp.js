// RegExp source and match offsets are observed in UTF-16 code units.

function units(text) {
  const result = [];
  for (let index = 0; index < text.length; index += 1) result.push(text.charCodeAt(index));
  return result;
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

const lonePattern = new RegExp("\\ud800(.)");
console.log("lone source", JSON.stringify(units(lonePattern.source)), lonePattern.flags);
check(lonePattern.source === "\\ud800(.)", "regexp source escapes lone high surrogate");
const loneMatch = lonePattern.exec("x" + String.fromCharCode(0xd800) + "Z");
check(loneMatch !== null && loneMatch.index === 1, "lone regexp offset");
check(loneMatch[0].charCodeAt(0) === 0xd800 && loneMatch[1] === "Z", "lone regexp capture");
console.log("lone match", loneMatch.index, JSON.stringify(units(loneMatch[0])), loneMatch[1]);

// A raw unit supplied at runtime must remain a unit in the canonical source and matcher.
const rawHighPattern = new RegExp(String.fromCharCode(0xd800) + "(.)");
const rawHighMatch = rawHighPattern.exec(String.fromCharCode(0xd800) + "Q");
check(rawHighPattern.source.charCodeAt(0) === 0xd800, "raw high source unit");
check(rawHighMatch !== null && rawHighMatch.index === 0 && rawHighMatch[1] === "Q",
      "raw high regexp match");
console.log("raw high", JSON.stringify(units(rawHighPattern.source)), rawHighMatch.index, rawHighMatch[1]);

// The astral character occupies two units, so the capture begins at unit offset 4.
const astralPattern = new RegExp("😀(Z)");
const astralMatch = astralPattern.exec("ab😀Z");
check(astralMatch !== null && astralMatch.index === 2, "astral regexp offset");
check(astralMatch[0].length === 3 && astralMatch[1] === "Z", "astral regexp capture");
console.log("astral match", astralMatch.index, astralMatch[0].length, astralMatch[1]);

const lowPattern = new RegExp("\\udc00", "g");
const lowSubject = "a" + String.fromCharCode(0xdc00) + "b";
const lowMatch = lowPattern.exec(lowSubject);
check(lowMatch !== null && lowMatch.index === 1 && lowMatch[0].charCodeAt(0) === 0xdc00,
      "lone low regexp");
console.log("low match", lowMatch.index, JSON.stringify(units(lowMatch[0])), lowPattern.global);
const rawLowPattern = new RegExp(String.fromCharCode(0xdc00));
const rawLowMatch = rawLowPattern.exec(String.fromCharCode(0xdc00));
check(rawLowPattern.source.charCodeAt(0) === 0xdc00 && rawLowMatch !== null,
      "raw low regexp source and match");
console.log("raw low", JSON.stringify(units(rawLowPattern.source)), rawLowMatch.index);
