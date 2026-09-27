// kiln-native: profile=native-release parallelism=1,2,max
//
// A typed array whose buffer was transferred away is detached: `map` and `parallelMap` over it
// throw a TypeError, and never read the bytes. Both fast paths read a view's length and bytes
// directly, and a detached buffer's view keeps its old length until something checks, so each
// path must check the buffer before trusting the view (realms issue 14 found the gap).
const buffer = new ArrayBuffer(64);
const view = new Float64Array(buffer);
for (let i = 0; i < view.length; i++) view[i] = i + 0.5;
console.log("before", view.parallelMap((x) => x * 2)[7], view.map((x) => x * 2)[7]);

const moved = buffer.transfer();
console.log("detached", buffer.detached, moved.byteLength);

for (const [label, run] of [
  ["parallelMap", () => view.parallelMap((x) => x * 2)],
  ["map", () => view.map((x) => x * 2)],
]) {
  try {
    const out = run();
    console.log(label, "answered", out.length);
  } catch (error) {
    console.log(label, "threw", error instanceof TypeError);
  }
}
