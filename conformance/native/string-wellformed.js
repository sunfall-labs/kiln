for (const text of ['', 'abc', '\ud83d\ude00', '\ud800', '\udc00', 'a\ud800b', '\ud800\ud800\udc00']) {
  console.log(text.isWellFormed(), JSON.stringify(text.toWellFormed()));
}
console.log(String.prototype.isWellFormed.call(123), String.prototype.toWellFormed.call(123));
for (const receiver of [null, undefined, Symbol('x')]) {
  try { String.prototype.isWellFormed.call(receiver); }
  catch (error) { console.log(error.name); }
}
console.log(String.prototype.isWellFormed.length, String.prototype.toWellFormed.length);
console.log(Object.getOwnPropertyDescriptor(String.prototype, 'isWellFormed').enumerable);
