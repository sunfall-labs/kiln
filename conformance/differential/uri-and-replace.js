// The four URI functions differ only in which characters they leave alone.
console.log(encodeURI("http://x/a b?c=d&e#f"));
console.log(encodeURIComponent("http://x/a b?c=d&e#f"));
console.log(decodeURI("http%3A%2F%2Fx%20y"), decodeURIComponent("http%3A%2F%2Fx%20y"));
console.log(encodeURI("héllo"), encodeURIComponent("héllo"));
console.log(decodeURIComponent(encodeURIComponent("héllo 😀")));
console.log(encodeURI("a~b!c*d'e(f)g-h_i.j"), encodeURIComponent("a~b!c*d'e(f)g-h_i.j"));
console.log(encodeURI(""), encodeURIComponent(1), decodeURI("plain"));
try { decodeURIComponent("%"); } catch (error) { console.log("malformed", typeof error.message); }

// A replacement is a template even when the pattern is a plain string.
console.log("a".replace("a", "$&$&"), "ab".replace("b", "[$`]"), "ab".replace("a", "[$']"));
console.log("x".replace("x", "$$"), "x".replace("x", "a$1b"), "x".replace("x", "$"));
console.log("abc".replaceAll("", "-"), "".replace("", "X"), "abc".replace("", "-"));
console.log("aXbXc".replaceAll("X", "$&$&"), "zz".replaceAll("z", "$`"));
console.log("nope".replace("zzz", "X"), "aaa".replaceAll("a", ""));

// The regexp path already agreed, and the two now share one expander.
console.log("ab".replace(/(a)(b)/, "$2$1"), "ab".replace(/a/, "$&$&"));
console.log("a1b2".replace(/\d/g, "[$&]"), "xy".replace(/(x)/, "$1$1$2"));
