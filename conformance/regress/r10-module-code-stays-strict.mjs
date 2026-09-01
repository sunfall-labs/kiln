// The other half of r09, and the one a fix for it can silently break: a module is strict by
// definition, whatever its text says. If sloppy mode is applied by strictness rather than by
// goal, an undeclared assignment starts creating globals here too -- which Test262 catches as
// losses across `language/module-code`, and which no bundle would ever have shown.
function receiver() {
	return typeof this;
}

console.log(receiver());

try {
	moduleUndeclared = 1;
	console.log('assigned');
} catch (error) {
	console.log(error.constructor.name);
}

console.log(typeof globalThis.moduleUndeclared);
