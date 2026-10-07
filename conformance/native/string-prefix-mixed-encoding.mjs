// kiln-native: profile=native-release
// One-argument prefix/suffix calls use the release backend's string fast path.
// A non-Latin1 code unit forces the receiver to UTF-16 while these prefixes/suffixes
// remain ASCII, exercising mixed internal string representations.
const prefixWide = "/é😀";
const suffixWide = "😀/";
console.log(
  prefixWide.startsWith("/"),
  prefixWide.startsWith("/é"),
  prefixWide.startsWith(""),
  prefixWide.endsWith("😀"),
  prefixWide.endsWith(""),
);
console.log(
  suffixWide.startsWith("😀"),
  suffixWide.startsWith(""),
  suffixWide.endsWith("/"),
  suffixWide.endsWith("😀/"),
  suffixWide.endsWith(""),
);
console.log(
  prefixWide.startsWith("x"),
  prefixWide.startsWith("/é😀x"),
  suffixWide.endsWith("x"),
  suffixWide.endsWith("x😀/"),
);

// Search inputs assembled at runtime should take the same code-unit path as literals.
const pieces = ["é", "😀", "/"];
const runtimeValue = pieces.join("");
const runtimePrefix = String.fromCharCode(0x00e9) + "😀";
const runtimeSuffix = String.fromCharCode(0x2f);
console.log(runtimeValue.startsWith(runtimePrefix), runtimeValue.endsWith(runtimeSuffix));

// startsWith/endsWith compare UTF-16 code units, including a prefix/suffix that splits a pair.
const pair = "😀";
console.log(
  pair.startsWith("\uD83D"),
  pair.endsWith("\uDE00"),
  pair.startsWith("\uDE00"),
  pair.endsWith("\uD83D"),
);
