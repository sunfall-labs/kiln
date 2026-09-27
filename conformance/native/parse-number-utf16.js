// Parallel-builtins issue 30, item 3. `parseInt` and `parseFloat` read a UTF-16 string (one that
// holds any unit past U+00FF) the way they read any other: leading white space, Unicode's
// included, then the longest prefix of the grammar. Natively they used to fail with
// "kiln: out of memory".
const inputs = ["42日", "3.5日", "日", "　  17px日", "﻿-0x1F日", "  ٣7", "1e3日", "-Infinity日",
                ".5€", "+.25e-1ℹ", "12  34", "0b11日", "€12", "", " 12"];
for (const s of inputs) {
  console.log(JSON.stringify(s), parseInt(s), parseInt(s, 16), parseInt(s, 36), parseFloat(s),
              Number.parseInt(s), Number.parseFloat(s));
}
console.log(parseInt({ toString: () => "77日" }), parseFloat(["9.5日"]));
