const base = { a: 1, b: 2 };
console.log(JSON.stringify({ ...base }));
// Order decides: a property after a spread wins, one before it loses.
console.log(JSON.stringify({ a: 0, ...base, b: 9 }));
console.log(JSON.stringify({ ...base, ...{ b: 3, c: 4 } }));
console.log(JSON.stringify({ ...null, ...undefined, ok: true }));
console.log(JSON.stringify({ ...[7, 8] }), JSON.stringify({ ..."hi" }));
// A computed key is evaluated in source order, between the members around it.
const order = [];
const key = (name) => {
	order.push(name);
	return name;
};
const built = { [key("x")]: 1, ...{ y: 2 }, [key("z")]: 3 };
console.log(order.join(","), JSON.stringify(built));
// Rest keeps what the named properties did not take.
const { a, ...rest } = { a: 1, b: 2, c: 3 };
console.log(a, JSON.stringify(rest));
const { missing, ...all } = base;
console.log(String(missing), JSON.stringify(all));
// Nested, renamed, and with a default.
const { b: renamed, ...others } = { b: 5, d: 6 };
console.log(renamed, JSON.stringify(others));
