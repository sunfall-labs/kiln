// `X.prototype` on the built-ins, `typeof` on what comes back, and the targets `Object.assign`
// accepts.
//
// WHY THESE ARE ONE PROBE. All three are the same question -- what a built-in *is* when a program
// handles it as a value rather than calling it -- and all three were answered by hand-written
// chains that had quietly stopped short.
//
// `typeof Function.prototype` was the worst of them. It is "function" in the language and in the
// interpreter, and this backend answered "object": no refusal, no diagnostic, just the other
// branch of the `typeof x === "function"` test real code uses to decide whether it may call
// something. A refusal would have been better, and a probe asking would have been better still.
//
// `Object.assign` refused whenever the target was a function, which is how a bundle attaches
// statics to the thing it exports -- `Object.assign(postcss, { parse, stringify })`. Only the
// target check rejected it; the write path had always worked, which is why `f.parse = parse` did.

const names = ["Object", "Array", "Function", "String", "Number", "Date", "RegExp", "Error",
  "TypeError", "Map", "Set", "WeakMap", "WeakSet", "Promise"];
for (const name of names) {
  const value = (globalThis as never as Record<string, { prototype: unknown }>)[name];
  console.log(name, typeof value.prototype);
}

// The target kinds, each with a value read back out, so a target that is accepted but not written
// to fails here rather than passing quietly.
function target() { return "called"; }
Object.assign(target, { parse: () => "parsed", count: 2 });
console.log(target(), (target as never as Record<string, () => string>).parse(), (target as never as Record<string, number>).count);

const array: Record<string, unknown> & unknown[] = [1, 2] as never;
Object.assign(array, { tag: "tagged" });
console.log(array.length, array.tag);

class Holder {}
Object.assign(Holder, { of: () => "static" });
console.log((Holder as never as Record<string, () => string>).of());

const plain = { kept: 1 };
Object.assign(plain, { added: 2 });
console.log(JSON.stringify(plain));
