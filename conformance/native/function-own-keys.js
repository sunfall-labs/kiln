// kiln-native: profile=native-release
"use strict";
function f(a, b) {}
const arrow = (a) => a;
class C {}
function* g() { yield 1; }
const bound = f.bind(null, 1);
for (const fn of [f, arrow, C, g, bound]) {
  fn.extra = 1;
  const s = Symbol("tag"); fn[s] = 2;
  console.log(Object.getOwnPropertyNames(fn).join(","), Object.keys(fn).join(","), Object.getOwnPropertySymbols(fn).length);
}
