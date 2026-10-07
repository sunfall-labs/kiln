class Words extends Array {
  toString() { return 'words:' + this.join('-'); }
  values() { return ['custom'][Symbol.iterator](); }
}
const words = new Words();
words.push('a', 'b');
console.log(words.toString());
console.log(words.toString.call(words));
console.log(words.values().next().value);
words.toString = function () { return 'own'; };
console.log(words.toString());
let reads = 0;
Object.defineProperty(Words.prototype, 'toString', { get() { reads++; return function () { return 'get'; }; } });
delete words.toString;
console.log(words.toString(), reads);
console.log(words.toString(), reads);
Object.defineProperty(Words.prototype, 'length', { value: 99 });
const plain = [1, 2];
Object.setPrototypeOf(plain, { length: 99 });
if (plain.length !== 2 || words.length !== 2) {
  throw new Error('an array own length must shadow a prototype length');
}
console.log(words.length);
