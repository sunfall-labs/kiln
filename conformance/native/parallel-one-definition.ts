// ADR 0068 rule 5: one definition. The parallel collection methods are the engine's own, on
// Array.prototype and %TypedArray%.prototype, named for their keys and non-enumerable; no
// package installs a fallback beside them.
const typedProto = Object.getPrototypeOf(Uint8Array.prototype);
console.log(Array.prototype.parallelMap.name, typedProto.parallelMap.name);
console.log([].parallelMap === Array.prototype.parallelMap);
const d = Object.getOwnPropertyDescriptor(Array.prototype, "parallelMap")!;
console.log(d.enumerable, d.writable, d.configurable);
const holey = [1, , 3] as number[];
console.log([1, 2, 3].parallelMap((x: number, i: number) => x + i).join(","), 1 in (holey as any).parallelMap((x: number) => x));
console.log(Array.from((new Int32Array([4, 5]) as any).parallelMap((x: number) => x * 10)).join(","));
