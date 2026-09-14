// Exercise a string that crosses the native boundary while allocation-triggered collection is
// enabled. The embedded NUL and non-ASCII bytes keep this from passing through an ASCII-only
// shortcut, and the later observations happen after enough unrelated allocations to collect the
// original parse/stringify intermediates.
const original = "é\u0000ÿ";
const round_tripped = JSON.parse(JSON.stringify({ value: original })).value;
for (let index = 0; index < 256; index += 1) {
  JSON.stringify({ index, filler: "allocation-" + index + "-" + original });
}
console.log(round_tripped.length);
console.log(round_tripped.charCodeAt(0), round_tripped.charCodeAt(1), round_tripped.charCodeAt(2));
