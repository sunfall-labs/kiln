const original = Temporal.Now;
const descriptor = Object.getOwnPropertyDescriptor(Temporal, "Now");

if (!descriptor || descriptor.value !== original || descriptor.writable !== true ||
    descriptor.enumerable !== false || descriptor.configurable !== true) {
  throw new Error("Temporal.Now descriptor does not match the replaceable builtin namespace member");
}

const replacement = { marker: 1 };
Object.defineProperty(Temporal, "Now", { value: replacement });
if (Temporal.Now !== replacement) throw new Error("Temporal.Now replacement was not visible");
Object.defineProperty(Temporal, "Now", descriptor);
if (Temporal.Now !== original) throw new Error("Temporal.Now descriptor did not restore the original value");

console.log("Temporal.Now descriptor replacement and restoration passed");
