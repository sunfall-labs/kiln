// Reducing an object to a primitive is a *call*, and which method is tried first depends on the
// operator: arithmetic wants a number and asks `valueOf` first, string conversion asks
// `toString` first. Every date and decimal library is used through the first of those.
const box = { v: 5, valueOf() { return this.v; } };
const other = { v: 2, valueOf() { return this.v; } };
console.log(box - other, box * other, box / other, box % other, box ** other);
console.log(box & 3, box | 8, box << 1, ~box, -box, +box);
console.log(box > other, box < other, box >= 5, box <= 5);
console.log(box == 5, box === 5, box != 5, box == other);

// `+` reduces with the default hint (valueOf first) where `${}` reduces with the string hint.
// They are not the same operation, which is why a template is not `"" + x`.
console.log(box + "", `${box}`, String(box));
const both = { valueOf() { return 1; }, toString() { return "two"; } };
console.log(both + "", `${both}`, both - 0, String(both), both == 1);

// A method that hands back another object is skipped: the point is to reach a primitive.
const stubborn = { valueOf() { return {}; }, toString() { return "reached"; } };
console.log(stubborn + "", stubborn - 0);

// The built-ins behave the same way, which is what makes date arithmetic work.
const start = new Date(400);
const end = new Date(1000);
console.log(end - start, end > start, typeof (end + ""));
console.log([2] - 1, [2] * 3, [] + [], String({}), `${{}}`);
console.log({} == {}, [1] == 1, [] == false, null == undefined);

// `x == null` answers without converting anything, so a wrapper's `valueOf` must not run --
// `value != null` is the commonest guard in JavaScript and reducing there is both wrong and,
// for a wrapper that assumes it is being used properly, fatal.
let ran = 0;
const guarded = {
	valueOf() {
		ran += 1;
		return 1;
	},
};
console.log(guarded == null, guarded != null, guarded == undefined, ran);

// A primitive answers `toString` and `valueOf` too, which library code reaches constantly on
// values it has already coerced.
console.log("ab".toString(), "ab".valueOf(), typeof "ab".toString);
console.log(true.toString(), false.toString(), true.valueOf(), typeof true.toString);
console.log((255).toString(16), (255).valueOf(), 1.5.toString());
