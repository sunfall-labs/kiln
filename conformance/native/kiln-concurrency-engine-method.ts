// ADR 0068 rule 5: one definition. `@kiln/concurrency` defines its `parallelMap` fallback only
// where the method is absent, so under kiln -- where both engines own it -- importing the
// package leaves the engine's method in place. The fallback is an anonymous function stored as
// `value:` and is therefore named "value"; the engine's is named for its key, and a fallback
// that overwrote it would show up here as a name that changed.
import "../../packages/kiln-concurrency/index.js";

const typedProto = Object.getPrototypeOf(Uint8Array.prototype);
console.log(Array.prototype.parallelMap.name, typedProto.parallelMap.name);
console.log([].parallelMap === Array.prototype.parallelMap);
const d = Object.getOwnPropertyDescriptor(Array.prototype, "parallelMap")!;
console.log(d.enumerable, d.writable, d.configurable);
const holey = [1, , 3] as number[];
console.log([1, 2, 3].parallelMap((x: number, i: number) => x + i).join(","), 1 in (holey as any).parallelMap((x: number) => x));
console.log(Array.from((new Int32Array([4, 5]) as any).parallelMap((x: number) => x * 10)).join(","));
