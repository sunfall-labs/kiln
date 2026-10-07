// kiln-native: profile=native-release
// Bracket \b denotes U+0008, including both endpoints of a range.
const patterns = [/^[\b]$/, /^[\0-\b]$/, /^[\b-\t]$/, /^[^\b]$/, /^[^"\\\0-\b\n-\x1F\x7F]/];
for (const pattern of patterns) {
  const accepted = [];
  for (let code = 0; code < 128; code++) {
    if (pattern.test(String.fromCharCode(code))) accepted.push(code);
  }
  console.log(JSON.stringify(accepted));
}
