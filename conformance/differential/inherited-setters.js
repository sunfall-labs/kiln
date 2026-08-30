// A setter found anywhere on the prototype chain takes the write. An assignment that skips the
// chain creates an own data property instead, which silently *shadows* the accessor: the write
// appears to work, the read appears to work, and the object behind the accessor never hears
// about it.
const proto = {};
Object.defineProperties(proto, {
	level: {
		enumerable: true,
		get() {
			return this.stored;
		},
		set(value) {
			this.stored = value * 2;
		},
	},
	readOnly: {
		get() {
			return "fixed";
		},
	},
});

const plain = Object.create(proto);
plain.level = 3;
console.log(plain.level, plain.stored, Object.hasOwn(plain, "level"));

// A callable is the case that libraries actually rely on: chalk and its like build a function
// and then link it to a prototype carrying every style as an accessor.
const callable = (...parts) => parts.join(" ");
Object.setPrototypeOf(callable, proto);
callable.level = 4;
console.log(callable.level, callable.stored, callable("a", "b"));

const array = [1, 2];
Object.setPrototypeOf(array, proto);
array.level = 5;
console.log(array.level, array.stored, array.length, array[0]);

// Getter-only swallows the write rather than shadowing it.
plain.readOnly = "ignored";
console.log(plain.readOnly, Object.hasOwn(plain, "readOnly"));

// `Object.create` takes the same descriptors as `defineProperties`, accessors included.
const made = Object.create(proto, {
	own: {
		enumerable: true,
		get() {
			return this.level + 1;
		},
	},
	data: { value: 9, enumerable: true, writable: true },
});
made.level = 10;
console.log(made.own, made.data, made.level, Object.keys(made).sort().join(","));
