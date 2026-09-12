// Regular expressions with capture group counts exceeding the runtime's fixed cache bound.
//
// `struct kiln_regexp` maintains an inline cache of size 32 (`cached_ranges[64]`,
// `cached_elements[32]`). Match operations that assumed group counts would never exceed 32
// passed `capacity = 64` (the array size in `int64_t` units) to `kiln_regexp_search`, which
// interpreted capacity as the number of capture group pairs (i.e. up to 128 elements). At 37
// capture groups (38 total groups including group zero), `kiln_regexp_search` wrote 76 int64_t
// values into the 64-element stack buffer, smashing the stack canary and aborting with SIGSEGV/SIGABRT.
//
// The native runtime safely limits fixed stack buffers to capacity / 2 groups and dynamically
// allocates heap storage whenever a pattern contains more than 32 groups.

for (const n of [32, 37, 48, 100]) {
  const re = new RegExp("(ab)".repeat(n));
  const s = "ab".repeat(n);
  const m = re.exec(s);
  console.log(n, m !== null, m ? m.length : 0, m ? m[0].length : 0, m ? m[n] : "", re.test(s));

  const splitParts = s.split(new RegExp("(ab)".repeat(n)));
  console.log("split", n, splitParts.length, splitParts[1]);

  const matchRes = s.match(new RegExp("(ab)".repeat(n)));
  console.log("match", n, matchRes !== null, matchRes ? matchRes.length : 0);
}

// Compile-time literal with 37 capture groups:
const lit37 = /(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)(ab)/;
const m37 = lit37.exec("ab".repeat(37));
console.log("literal 37:", m37 !== null, m37 ? m37.length : 0, m37 ? m37[37] : "", lit37.test("ab".repeat(37)));

