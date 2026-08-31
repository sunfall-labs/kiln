// Two things a program does to an object that are not writes to it: freezing it, and naming
// something that is not there.

// A frozen object refuses every write, and a module is strict code -- so the refusal is a
// TypeError, both for a property that is there and cannot change and for one that is not there
// and cannot be added. Nothing lands either way; strictness only decides whether it is said.
const settings = { retries: 3, verbose: false };
console.log(Object.freeze(settings) === settings, Object.isFrozen(settings));
function refused(attempt: () => void): string {
  try { attempt(); return "no throw"; } catch (error: any) { return error.constructor.name; }
}
console.log(refused(() => { settings.retries = 99; }), refused(() => { (settings as any).added = 1; }));
console.log(settings.retries, (settings as any).added, JSON.stringify(settings));
console.log(Object.isFrozen({}), Object.isFrozen(1), Object.isFrozen("x"), Object.freeze(7));
// Freezing is shallow: the object is frozen, not what it holds.
const nested = Object.freeze({ inner: { count: 0 } });
nested.inner.count = 5;
console.log(nested.inner.count, Object.isFrozen(nested.inner));

// A name that is not defined is a ReferenceError naming itself, and only when control reaches
// it -- a reference in a branch that never runs is not an error, which is why real bundles are
// full of them.
if (typeof globalThisIsAbsent !== "undefined") {
  console.log("never", (globalThisIsAbsent as any).x);
}
try {
  console.log(absentEverywhere);
} catch (error: any) {
  console.log(error.constructor.name, error.name, error.message);
}
// Caught by an enclosing handler rather than the nearest one, and inside a function.
function reach(): string {
  try {
    return alsoAbsent as unknown as string;
  } catch (error: any) {
    return `caught ${error.message}`;
  }
}
console.log(reach());
console.log(typeof stillAbsent, typeof reach);

// The realm has one global object, and something stored on it is found again. Its own names are
// non-enumerable, which is what node does and what stops `Object.keys(globalThis)` from being a
// list of the realm's furniture -- a program's own additions are enumerable, which is the
// distinction that makes the question worth asking.
(globalThis as any).__probe = { count: 0 };
(globalThis as any).__probe.count += 1;
console.log(globalThis === globalThis, (globalThis as any).__probe.count);
console.log(typeof globalThis, (globalThis as any).Object === Object, (globalThis as any).Math === Math);
console.log((globalThis as any).parseInt === parseInt, (globalThis as any).globalThis === globalThis);
// What the realm holds beyond the language's own names is the host's business, so only the
// program's own addition is asked about here.
console.log(Object.keys(globalThis).includes("__probe"), typeof (globalThis as any).NaN);
let holder: any;
(holder = globalThis).__lazy ?? (holder.__lazy = "made");
console.log((globalThis as any).__lazy);

// A class instance shows the name of the class that made it, which is how a console tells two
// objects of the same shape apart. A plain object's constructor is `Object`, and naming that
// would be noise rather than information.
class Point3 { x: number; constructor(x: number) { this.x = x; } }
console.log(new Point3(1), { x: 1 }, new Point3(2).x);
