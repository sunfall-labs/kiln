// A function declaration is usable before the line that writes it, and its body may refer to a
// binding declared below it -- the two together are why the block's *names* are created first
// and only the function *values* are emitted early.
// The body reads a `const` written below it, which is legal because the call happens after that
// line has run -- calling `top()` above it is a ReferenceError in both implementations.
function top() { return later() + ':' + CONSTANT; }
function later() { return 'later'; }
const CONSTANT = 'const';
console.log(top());

// The same inside a function, and inside a block.
function outer() {
	return inner();
	function inner() { return 'inner'; }
}
{
	console.log(outer(), blockScoped());
	function blockScoped() { return 'block'; }
}

// `var` is initialised to undefined when the scope is entered rather than throwing, which is the
// whole difference from `let` for hoisting purposes.
console.log(typeof hoistedVar, typeof hoistedFunction);
var hoistedVar = 1;
function hoistedFunction() {}

// An enclosing function's binding shadows a module binding of the same name. Reading the module
// one here would be wrong, and was: resolution used to jump straight from the inner function to
// module scope, skipping every function in between.
function shadowing({ value }) {
	return () => value;
}
console.log(shadowing({ value: 'parameter' })());
const value = 'module';
console.log(value);

// A destructured parameter shadows an outer binding of the same name too.
function plain({ value }) { return value; }
console.log(plain({ value: 'inner' }), value);
