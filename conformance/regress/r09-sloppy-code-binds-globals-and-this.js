// Sloppy code is not "strict with the throwing turned off": two of its rules decide whether a
// real npm package can install itself at all, and kiln had neither.
//
// An undeclared assignment creates a property on the global object. That is how a UMD header
// attaches its module -- `root.myLib = factory()` after `root` was found by assigning to a bare
// name -- and kiln raised a ReferenceError instead, which no guard catches because the header
// is not written to expect one.
//
// `this` in a bare call is the global object. That is how `var root = this || global` finds the
// realm, and kiln answered `undefined`, which sends the header down the branch for a stripped
// environment.
//
// The two rules the same code must NOT lose are pinned below them: reading a name nothing bound
// is still a ReferenceError (ADR 0017), and so is the read half of a compound assignment to one.
function receiver() {
	return typeof this;
}

function installs() {
	umdRoot = 'installed';
}

installs();
console.log(receiver(), typeof globalThis.umdRoot, globalThis.umdRoot);

// The name the assignment created resolves as a bare identifier afterwards, which is the half a
// header depends on: it writes `root` once and reads it everywhere.
console.log(umdRoot);

var root = (function () {
	return this;
})();
console.log(root === globalThis);

// A compound assignment reads before it writes, and reading a name nothing bound throws in
// sloppy code exactly as it does in strict code.
try {
	notThere += 1;
	console.log('compound: no throw');
} catch (error) {
	console.log('compound:', error.constructor.name);
}

try {
	alsoNotThere;
	console.log('read: no throw');
} catch (error) {
	console.log('read:', error.constructor.name);
}
