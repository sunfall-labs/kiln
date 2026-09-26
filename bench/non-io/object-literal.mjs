import { bench } from "../harness.mjs";

// Object literals with static keys: one, three and eight members, and a nested one shaped like
// a `Response` init. Each result is read once so the literal cannot be optimized away.
function one(i) {
  return { a: i };
}
function three(i) {
  return { a: i, b: "x", c: true };
}
function eight(i) {
  return { a: i, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8 };
}
function nested(i) {
  return { status: i, headers: { "content-type": "text/plain" } };
}

bench("object-literal", () => {
  let count = 0;
  for (let i = 0; i < 20000; i++) {
    count += one(i).a;
    count += three(i).a;
    count += eight(i).h;
    count += nested(i).status;
  }
  return count;
});
