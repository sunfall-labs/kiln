// The native global is assembled from many built-in names. Its descriptor writer must preserve
// the final attributes, key order, and realm-owned values before and after ordinary mutations.
const names = ["Object", "parseInt", "console", "NaN", "Infinity", "undefined", "globalThis"];
const before = Reflect.ownKeys(globalThis);
for (const name of names) {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, name)!;
  console.log(name, typeof descriptor.value, descriptor.enumerable,
    descriptor.writable, descriptor.configurable,
    before.includes(name));
}

const original = globalThis.parseInt;
globalThis.parseInt = ((text: string) => Number(text) + 1) as typeof parseInt;
console.log(globalThis.parseInt("4"), Object.getOwnPropertyDescriptor(globalThis, "parseInt")!.enumerable);
globalThis.parseInt = original;

try { (globalThis as any).NaN = 1; console.log("NaN wrote"); }
catch (error) { console.log("NaN refused", error instanceof TypeError); }
console.log(Number.isNaN(globalThis.NaN), Object.keys(globalThis).includes("parseInt"));
const after = Reflect.ownKeys(globalThis);
console.log(before.length === after.length && before.every((key, index) => key === after[index]));
