// The string methods the native backend did not have, and the one place a lone surrogate has an
// answer: `codePointAt` returns a *number*, so there is no string to be unable to hold one.

const text = "héllo 😀 world";
console.log(text.codePointAt(0), text.codePointAt(1), text.codePointAt(6));
// Index 7 is the trailing half of the emoji. `charCodeAt` and `codePointAt` agree there, because
// a lone surrogate is not a pair -- reading it through a string would answer U+FFFD instead.
console.log(text.codePointAt(7), text.charCodeAt(7), text.codePointAt(99));
console.log("abc".codePointAt(-1), "".codePointAt(0), "😀".codePointAt(0), "😀".charCodeAt(0));

// A negative start counts from the end, which is the one thing `substr` does that `substring`
// does not.
console.log(text.substr(0, 5), text.substr(-5), text.substr(-5, 2), text.substr(2));
console.log("abc".substr(1), "abc".substr(1, 1), "abc".substr(9), "abc".substr(0, 99));
console.log("abc".substr(0, -1) === "", "abc".substr(-99), "abc".substr(1, 0) === "");

// No collation tables on either side, so this compares code units -- which is a valid locale
// ordering and the one both engines use.
console.log("a".localeCompare("b"), "b".localeCompare("a"), "a".localeCompare("a"));
console.log(["b", "a", "c"].sort((l, r) => l.localeCompare(r)).join(""));

// No normalisation tables either, so `normalize` is the identity: honest for text that is already
// normalised, which is what a bundler emits.
console.log("abc".normalize(), "abc".normalize("NFC"), "abc".toString(), "abc".valueOf());
console.log(text.toString() === text, text.valueOf() === text, "".normalize() === "");
console.log("".normalize("NFKD") === "", "ﬁ".normalize("NFKD"), "e\u0301".normalize("NFC"));
try { "".normalize("INVALID"); } catch (e) { console.log(e instanceof RangeError); }
try { "abc".normalize("INVALID"); } catch (e) { console.log(e instanceof RangeError); }

const probe = "example_text";
console.log(probe.localeCompare(probe), "hello".localeCompare("hello"), "".localeCompare(""));
console.log(["b", "a", "C"].sort((x, y) => x.localeCompare(y, "en", { sensitivity: "base" })).join(""));
try { "a".localeCompare("a", "invalid!!"); } catch (e) { console.log(e instanceof RangeError); }
