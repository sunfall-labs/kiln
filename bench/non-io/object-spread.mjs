import { bench } from "../harness.mjs";

const base = { a: 1, b: "hello", c: true, d: [1, 2], e: 42 };

bench("object-spread", () => {
  let count = 0;
  for (let i = 0; i < 50000; i++) {
    const merged = { ...base, f: i, g: i * 2 };
    count += merged.f + merged.a;
  }
  return count;
});
