// JSON.stringify: what it writes, what it drops, and what it refuses.

const order = {
	id: 7,
	customer: "Ada",
	total: 12.5,
	paid: false,
	shippedAt: null,
	items: ["widget", "gasket"],
};
console.log(JSON.stringify(order));

// A property with no JSON spelling is dropped; an array element becomes null, because dropping
// it would renumber everything after it.
console.log(JSON.stringify({ kept: 1, gone: undefined, fn: function () {} }));
console.log(JSON.stringify([1, undefined, 3]));

// Neither NaN nor an infinity is expressible, so both are written as null.
console.log(JSON.stringify([0 / 0, 1 / 0]));

// Strings are escaped to the JSON grammar, control characters included.
console.log(JSON.stringify('he said "hi"\tthen\nleft'));

// The third argument indents. A number is that many spaces, a string is used as-is.
console.log(JSON.stringify({ a: [1, 2], b: {} }, null, 2));

// The same object twice is a diamond, not a cycle, and serialises fine both times.
const shared = { n: 1 };
console.log(JSON.stringify([shared, shared]));
