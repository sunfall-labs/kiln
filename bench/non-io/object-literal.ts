import { bench } from "../harness.mjs";

interface One {
  a: number;
}
interface Three {
  a: number;
  b: string;
  c: boolean;
}
interface Eight {
  a: number;
  b: number;
  c: number;
  d: number;
  e: number;
  f: number;
  g: number;
  h: number;
}
interface Nested {
  status: number;
  headers: { "content-type": string };
}

// Object literals with static keys: one, three and eight members, and a nested one shaped like
// a `Response` init. Each result is read once so the literal cannot be optimized away.
function one(i: number): One {
  return { a: i };
}
function three(i: number): Three {
  return { a: i, b: "x", c: true };
}
function eight(i: number): Eight {
  return { a: i, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8 };
}
function nested(i: number): Nested {
  return { status: i, headers: { "content-type": "text/plain" } };
}

bench("object-literal", (): number => {
  let count: number = 0;
  for (let i: number = 0; i < 20000; i++) {
    count += one(i).a;
    count += three(i).a;
    count += eight(i).h;
    count += nested(i).status;
  }
  return count;
});
