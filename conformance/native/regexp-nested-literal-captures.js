// kiln-native: profile=native-release
// Nested captures must lower completely in native-release's literal matcher.
const cases = [
  [/^((a(b)?))$/, 'a'],
  [/^((a(b)?))$/, 'ab'],
  [/^((()))$/, ''],
  [/(a(b))(c)/, 'zabc'],
  [/((ab))(c)/g, 'zabcabc'],
];
for (const entry of cases) {
  const re = entry[0];
  const match = re.exec(entry[1]);
  console.log(JSON.stringify(match), match === null ? -1 : match.index, re.lastIndex);
}
