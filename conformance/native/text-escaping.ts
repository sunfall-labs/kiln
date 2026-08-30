// The six global functions that turn text into text: `escape`/`unescape` from Annex B, and the
// four URI functions.
//
// They are here rather than in the differential corpus alone because the two engines encode
// strings differently on the way in -- the interpreter holds a Rust `String` and the backend
// holds UTF-8 bytes it walks itself -- and these are the functions where that difference would
// show. `escape` works in UTF-16 code units and `encodeURI` works in UTF-8 bytes, and a probe
// that only used ASCII would not tell them apart.

// The four differ only in which characters they leave alone.
console.log(encodeURI("http://x/a b?c=d&e#f"));
console.log(encodeURIComponent("http://x/a b?c=d&e#f"));
console.log(decodeURI("http%3A%2F%2Fx%20y"), decodeURIComponent("http%3A%2F%2Fx%20y"));
console.log(encodeURI("héllo"), encodeURIComponent("héllo"));
console.log(decodeURIComponent(encodeURIComponent("héllo 😀")));
console.log(encodeURI("a~b!c*d'e(f)g-h_i.j"), encodeURIComponent("a~b!c*d'e(f)g-h_i.j"));
console.log(encodeURI(""), encodeURIComponent(1), decodeURI("plain"), encodeURI(true));

// A `%` that does not introduce two hex digits is malformed here, and stands for itself in
// `unescape` -- which is the whole difference between the two families' error handling.
try { decodeURIComponent("%"); } catch (error: any) { console.log("malformed", typeof error.message); }
try { decodeURI("%zz"); } catch (error: any) { console.log("malformed", typeof error.message); }
// Escapes that name bytes which are not a string are malformed too.
try { decodeURIComponent("%FF"); } catch (error: any) { console.log("bytes", typeof error.message); }

// Two hex digits below 256, four with a `%u` prefix above it -- the split that got `escape`
// replaced, and the reason `unescape(encodeURIComponent(s))` is how a library reads UTF-8 bytes.
console.log(escape("a b/c?d"), escape("héllo"), escape("😀"));
console.log(unescape("a%20b"), unescape("h%E9llo"), unescape("%uD83D%uDE00"));
console.log(unescape(encodeURIComponent("héllo")).length, "héllo".length);
console.log(escape(""), unescape(""), unescape("%"), unescape("%zz"), unescape("%2"));
console.log(escape("@*_+-./"), escape("!~'()"), escape(1), unescape("%u00"));
const roundTrip = unescape(encodeURIComponent("kiln"));
console.log(roundTrip, roundTrip.length, escape, unescape, decodeURI);
