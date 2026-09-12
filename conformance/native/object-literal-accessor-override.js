// An object literal's member defines a property (`CreateDataPropertyOrThrow`):
// it replaces whatever own property the name had, accessor included.
// Defining a plain data member after a getter or setter of the same name must
// replace the accessor, leave setter uncalled, and produce a writable, enumerable,
// configurable data property.

// 1. Plain member after getter and setter
const lit1 = { get w() { return 1; }, w: 2, set u(x) {}, u: 3 };
console.log('lit1:', lit1.w, lit1.u, JSON.stringify(Object.getOwnPropertyDescriptor(lit1, 'u')));

// 2. Member before getter, then member after getter
const lit2 = { a: 1, get a() { return 2; }, a: 3 };
console.log('lit2:', lit2.a, JSON.stringify(Object.getOwnPropertyDescriptor(lit2, 'a')));

// 3. Setter must not be invoked by subsequent definition
let setterCalled = false;
const lit3 = { set x(v) { setterCalled = true; }, x: 42 };
console.log('lit3:', lit3.x, setterCalled, JSON.stringify(Object.getOwnPropertyDescriptor(lit3, 'x')));

// 4. Computed key after getter
const k = 'computed_key';
const lit4 = { get [k]() { return 10; }, [k]: 20 };
console.log('lit4:', lit4[k], JSON.stringify(Object.getOwnPropertyDescriptor(lit4, k)));

// 5. Symbol key after getter
const sym = Symbol('sym');
const lit5 = { get [sym]() { return 'sym-get'; }, [sym]: 'sym-val' };
console.log('lit5:', lit5[sym], JSON.stringify(Object.getOwnPropertyDescriptor(lit5, sym)));

// 6. Function value after getter names the function
const lit6 = { get f() { return 1; }, f() { return 2; } };
console.log('lit6:', typeof lit6.f, lit6.f.name, lit6.f(), JSON.stringify(Object.getOwnPropertyDescriptor(lit6, 'f')));

// 7. Key order preserved
const lit7 = { a: 1, get b() { return 2; }, b: 3, c: 4 };
console.log('lit7 keys:', Object.keys(lit7).join(','));
