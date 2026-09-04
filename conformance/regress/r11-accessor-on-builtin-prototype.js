// An accessor a program defines on a built-in prototype, read through an instance.
//
// Nothing links to a built-in prototype: the tables are match arms rather than property stores,
// and the store walk stops at the last stored link. A *data* property defined on one was still
// answered, because the read falls through to the table and the table consults the realm's store
// on the way past -- so `Array.prototype.f = g` worked and polyfills looked fine. An accessor
// could not be, because it is a call and reaching it needs the property rather than the value.
//
// That asymmetry read as a wrong answer, never a refusal. The expensive case was `constructor`:
// species construction asks an *instance* for `.constructor`, a program that installs one
// installs an accessor, and the instance answered the table's constructor while the prototype
// answered the override. Every `@@species` path was reading a name the program had replaced.
//
// Primitives are here for a second reason: the chain a read walks is not the chain
// `getPrototypeOf` reports. `"s".at` finds `String.prototype.at` with no wrapper made anywhere,
// so a read starts at the wrapper prototype where reflection answers null -- and reading the two
// off one function left strings and numbers behind when every other kind had been fixed.
const failures = [];
const check = (what, got, want) => {
	if (got !== want) failures.push(`${what}: ${String(got)} !== ${String(want)}`);
};

const cases = [
	['Array', Array.prototype, () => []],
	['String', String.prototype, () => 'ab'],
	['Number', Number.prototype, () => 1],
	['Boolean', Boolean.prototype, () => true],
	['Map', Map.prototype, () => new Map()],
	['Set', Set.prototype, () => new Set()],
	['RegExp', RegExp.prototype, () => /x/],
	['Promise', Promise.prototype, () => Promise.resolve(1)],
	['Date', Date.prototype, () => new Date(0)],
	['Error', Error.prototype, () => new Error('e')],
	['Int8Array', Int8Array.prototype, () => new Int8Array(1)],
	['Object', Object.prototype, () => ({})],
];

for (const [name, prototype, make] of cases) {
	let reads = 0;
	Object.defineProperty(prototype, 'probeAccessor', {
		get() {
			reads += 1;
			return 'from accessor';
		},
		configurable: true,
	});
	check(`${name} instance`, make().probeAccessor, 'from accessor');
	check(`${name} prototype`, prototype.probeAccessor, 'from accessor');
	check(`${name} reads`, reads, 2);
	// The computed spelling walks the same chain. It used to walk a second copy of it.
	const key = 'probeAccessor';
	check(`${name} computed`, make()[key], 'from accessor');
	delete prototype.probeAccessor;
	check(`${name} after delete`, make().probeAccessor, undefined);
}

// An own property still shadows an inherited accessor, and a dense element still shadows an
// accessor on `Array.prototype` that shares its index. Both are the direction a chain does not
// run in, and both were reachable the moment the walk above existed at all.
Object.defineProperty(Object.prototype, 'shadowed', { get: () => 'inherited', configurable: true });
check('own shadows inherited', { shadowed: 'own' }.shadowed, 'own');
delete Object.prototype.shadowed;

Object.defineProperty(Array.prototype, '0', { get: () => 'inherited', configurable: true });
check('element shadows inherited', ['element'][0], 'element');
check('hole reaches inherited', [, 'x'][0], 'inherited');
delete Array.prototype[0];

// `constructor` redefined as an accessor is the case that cost the species paths.
let constructorReads = 0;
Object.defineProperty(Array.prototype, 'constructor', {
	get() {
		constructorReads += 1;
		return Array;
	},
	configurable: true,
});
check('species maps', [1, 2].map((x) => x).length, 2);
check('species read the accessor', constructorReads > 0, true);
delete Array.prototype.constructor;
Object.defineProperty(Array.prototype, 'constructor', {
	value: Array,
	writable: true,
	configurable: true,
});

// Two rules the walk above must not cross, both of which it broke on the way in.
//
// A view's `[[Get]]` answers a canonical numeric index itself, present or not: `"1.1"`, `"-0"`
// and an index past the end are `undefined` with the chain never consulted at all.
const view = new Int8Array([42, 43]);
Object.defineProperty(Int8Array.prototype, '1.1', {
	get: () => 'from prototype',
	configurable: true,
});
check('view skips the chain for a numeric key', view['1.1'], undefined);
check('view still reads its elements', view[1], 43);
delete Int8Array.prototype['1.1'];

// A `String.prototype` method looks its symbol up on the argument only once it knows the
// argument is an Object. A primitive is never asked, and an accessor is how you can tell.
//
// This one line is deliberately *not* node's answer. Looking the symbol up only on an Object is
// a normative change node has not shipped -- run this file under node and it reports the read --
// and the pinned Test262 asserts the newer rule across six methods and four primitive types.
// The corpus is the standard being measured against, so the corpus wins; the note is here so
// that the next person to diff this file against node finds the reason instead of a mystery.
Object.defineProperty(Number.prototype, Symbol.match, {
	get() {
		failures.push('@@match was read on a number primitive');
		return undefined;
	},
	configurable: true,
});
check('match on a primitive', 'a1b1c'.match(1)[0], '1');
delete Number.prototype[Symbol.match];

console.log(failures.length === 0 ? 'ok' : failures.join('\n'));
if (failures.length !== 0) throw new Error(`${failures.length} accessor(s) unreachable`);
