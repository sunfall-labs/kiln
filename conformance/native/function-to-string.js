// `Function.prototype.toString` reaches the two engines by two different routes, and for a long
// time only one of them worked natively. `String(f)` and `"" + f` go through the text conversion,
// which the backend implements; `f.toString()` is a *property* read, and there was no `toString`
// among the members a callable answers to -- so it read `undefined` and the call failed with
// "undefined is not a function". A wrong answer rather than a refusal, on a value whose other two
// spellings were already right, which is why `scripts/native.sh` never saw it: no probe here
// called the method.
//
// All three spellings must agree, for each shape, on both engines.
function user(a, b) {
	return a + b;
}
class Shape {
	area(w) {
		return w * w;
	}
	get side() {
		return 1;
	}
}
const literal = {
	meth(z) {
		return z;
	},
};

console.log(String(user));
console.log('' + user);
console.log(user.toString());
console.log(String(Shape));
console.log(Shape.toString());
console.log(Shape.prototype.area.toString());
console.log(literal.meth.toString());

// A built-in must produce the NativeFunction form instead, because that is what feature
// detection reads. Getting this backwards is worse than the honest placeholder it replaced.
console.log(Math.max.toString());
console.log(Object.keys.toString());
console.log(String(Object));

const native = /\{\s*\[native code\]\s*\}/;
console.log('detect', native.test(Math.max.toString()), native.test(user.toString()));

// An arrow and a bound function, which take different routes to the same answer.
const arrow = (x) => x * 2;
console.log(arrow.toString());
console.log(user.bind(null).toString());
