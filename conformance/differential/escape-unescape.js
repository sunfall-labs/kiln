// `escape` and `unescape` are the pre-Unicode pair the specification keeps in Annex B. Real code
// still reaches for them: `unescape(encodeURIComponent(s))` is how a library gets a string's
// UTF-8 bytes one per code unit.
console.log(escape("a b/c?d"), escape("héllo"), escape("😀"));
console.log(unescape("a%20b"), unescape("h%E9llo"), unescape("%uD83D%uDE00"));
console.log(unescape(encodeURIComponent("héllo")).length, "héllo".length);
console.log(escape(""), unescape(""), unescape("%"), unescape("%zz"), unescape("%2"));
console.log(escape("@*_+-./"), escape("!~'()"));
const roundTrip = unescape(encodeURIComponent("kiln"));
console.log(roundTrip, roundTrip.length);
