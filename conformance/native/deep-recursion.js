// Parallel-builtins issue 30, item 4. A recursion 5,000 calls deep finishes natively as it does
// in the interpreter and node: plain recursion, a method recursing through its receiver chain, a
// recursion through `Function.prototype.call` (150 deep: the interpreter re-enters itself for
// each and bounds that at 192), and one whose frames carry locals, a `try` and a
// `for...of`. The native limit was 2,048 calls; it is now the interpreter's 10,000, with the
// stack to hold them. A recursion that never ends is still a catchable `RangeError`, and the
// program goes on afterwards.
function down(n) { return n === 0 ? 0 : 1 + down(n - 1); }
class Link {
  constructor(value, next) { this.value = value; this.next = next; }
  total() { return this.value + (this.next === null ? 0 : this.next.total()); }
}
function viaCall(n) { return n === 0 ? 0 : 1 + viaCall.call(null, n - 1); }
function heavy(n, text) {
  const parts = [n, text.length];
  let sum = 0;
  try {
    for (const part of parts) sum += part;
    if (n > 0) sum += heavy(n - 1, text + "");
  } catch (error) {
    throw error;
  }
  return sum;
}
console.log("down", down(5000));
let chain = null;
for (let i = 0; i < 5000; i++) chain = new Link(1, chain);
console.log("total", chain.total());
console.log("call", viaCall(150));
console.log("heavy", heavy(5000, "ab"));
function forever(n) { return forever(n + 1) + 1; }
for (let attempt = 0; attempt < 2; attempt++) {
  try { forever(0); } catch (error) { console.log("forever", error instanceof RangeError); }
}
console.log("after", down(100));
