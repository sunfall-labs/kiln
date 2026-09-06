// The cases the ASCII fast path in strings.c declines, so they fall through to the UTF-16-unit
// path unchanged: a regexp separator, a non-ASCII receiver split on an ASCII separator, and an
// ASCII receiver split on a non-ASCII separator. (A receiver holding a surrogate pair split on
// an empty separator is deliberately not asserted here: that already disagrees between this
// backend's own interpreter and its native compiler before this change -- ADR 0022's lone
// surrogate becoming U+FFFD is one divergence, and the interpreter additionally treats the pair
// as one code point rather than two units, which is a second, older one. Neither receiver here is
// ASCII, so the fast path this file is testing declines both and this probe cannot tell them
// apart from the pre-existing behavior; scripts/native.sh already runs the two engines against
// each other and would fail on that pair for a reason this change did not create.)
console.log("a1b2c3".split(/\d/));
console.log("café au lait".split(" "));
console.log("héllo".split("é"));
console.log("a😀b😀c".split("😀"));
