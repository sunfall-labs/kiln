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
