// The only sloppy code in this corpus, and the only route sloppy code has to the native backend.
//
// `kiln run` compiles a `.ts` or `.js` entry as ESM, which is strict, so every other probe here
// asks the two engines a question about strict code. These are the two rules that differ, and
// both of them are what a UMD header depends on: an assignment to a name nothing bound creates a
// property on the global object, and `this` in a bare call is that object.
//
// The interpreter reads its answer off the compiler's strictness table; the native backend has no
// table, so the bit travels on the closure. Two mechanisms for one rule is exactly the shape that
// drifts, which is why it is gated here.
function receiver() {
	return typeof this;
}

function installs() {
	umdRoot = 'installed';
}

installs();
console.log(receiver(), typeof globalThis.umdRoot, globalThis.umdRoot, umdRoot);

var root = (function () {
	return this;
})();
console.log(root === globalThis);

// The write a defensive library makes and does not expect to throw, which is sloppy in both
// engines already -- kept beside the two above because it is the reason the strictness of a
// CommonJS body was never uniform in the first place.
var frozen = Object.freeze({ x: 1 });
frozen.x = 2;
console.log(frozen.x);
