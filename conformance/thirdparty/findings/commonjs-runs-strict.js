// Kiln compiles CommonJS as strict code; node runs it sloppy.
//
// Found while giving sloppy code the receiver `OrdinaryCallBindThis` specifies. That rule landed
// and Test262 gained 201 variants, but not one of the real-world corpora moved -- because there
// is no sloppy code in them to move. Every bundle here is CommonJS, and kiln compiles CommonJS
// strict, so the rule that had just been fixed was unreachable from any real program.
//
// This is the difference the strictness of a CommonJS module actually makes, and each line is a
// question a real bundle asks:
//
//   * an undeclared assignment creates a global in node and throws here, which is how a UMD
//     header installs itself;
//   * `this` in a bare call is the global object in node and `undefined` here, which is how
//     `var root = this || global` finds the global;
//   * a write to a frozen property is silently discarded by *both*, which is the interesting
//     part: the strictness is not uniform. The binding rules are strict here and the refused
//     write is not, so this is not simply "kiln runs CommonJS strict" -- it is strict in the two
//     places a UMD header notices and sloppy in the place a defensive library does.
//
// node:  sloppy                | object    | discarded
// kiln:  strict:ReferenceError | undefined | discarded
function mode() {
	try {
		undeclaredGlobal = 1;
		return 'sloppy';
	} catch (error) {
		return 'strict:' + error.constructor.name;
	}
}
function receiver() {
	return typeof this;
}
function assignsToFrozen() {
	const frozen = Object.freeze({ x: 1 });
	try {
		frozen.x = 2;
		return 'discarded';
	} catch (error) {
		return error.constructor.name;
	}
}
console.log(mode(), receiver(), assignsToFrozen());
