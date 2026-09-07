// `parallelMap` is described as a member of `Array.prototype` on both engines (ADR 0054), so a
// read answers a callable, a feature test takes the same branch as on the interpreter, and a
// call through the read value is `map` under its own name -- generic, like the one borrowed from
// `Array.prototype`. `.length` is not read here: a native method's own `length` is a deliberate
// refusal on this backend (`builtin-length-refuses`), for every method, and this probe is about
// the member existing rather than about that. Two more things it deliberately does not compare:
// `[].map === Array.prototype.map` (false here, true on the interpreter: the prototype read hands
// out the generic code, a divergence older than this probe), and `Array.prototype[Symbol.unscopables]`
// (absent here; `.scratch/array-unscopables` holds the plan). `parallelMap` sits in neither list
// on the interpreter, so there is nothing for this member to agree on there yet.
const arr = [1, 2, 3];
console.log(typeof [].parallelMap, typeof Array.prototype.parallelMap, typeof [].map);
console.log([].parallelMap === Array.prototype.parallelMap);
const d = Object.getOwnPropertyDescriptor(Array.prototype, "parallelMap");
const m = Object.getOwnPropertyDescriptor(Array.prototype, "map");
console.log(d ? [typeof d.value, d.writable, d.enumerable, d.configurable].join(",") : "absent");
console.log(m ? [typeof m.value, m.writable, m.enumerable, m.configurable].join(",") : "absent");
console.log(Array.prototype.parallelMap.name, Array.prototype.map.name);
console.log(Object.prototype.hasOwnProperty.call(Array.prototype, "parallelMap"), Object.getOwnPropertyNames(Array.prototype).includes("parallelMap"));
// A call through a read value, three ways.
const f = [].parallelMap;
console.log(f.call(arr, (x: number) => x * 2).join(","));
console.log(arr.parallelMap.call(arr, (x: number, i: number) => x + i).join(","));
console.log(Reflect.apply(Array.prototype.parallelMap, arr, [(x: number) => -x]).join(","));
// Generic: borrowed onto a typed view or an array-like it walks the receiver like map does. What
// comes back from a view is not compared: `Array.prototype.map.call(view)` is an Array here and a
// view on the interpreter, a divergence of map's that parallelMap inherits on each engine.
const view = new Float64Array([1.5, 2.5]);
const fromView = Array.prototype.parallelMap.call(view, (x: number) => x * 2);
console.log(fromView.length, fromView[0], fromView[1], typeof (view as any).parallelMap);
console.log(Array.prototype.parallelMap.call({ length: 2, 0: "a", 1: "b" }, (x: string) => x + x).join(","));
// The direct call site, holes kept, receiver passed, index passed.
console.log(arr.parallelMap((x: number) => x * 3).join(","), [1, , 3].parallelMap((x: number) => x).length, 1 in [1, , 3].parallelMap((x: number) => x));
console.log(arr.parallelMap((x: number, i: number, a: number[]) => a === arr ? i : -1).join(","));
