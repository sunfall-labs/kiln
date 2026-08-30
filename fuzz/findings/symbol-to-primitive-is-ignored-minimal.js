// The one-line form of `symbol-to-primitive-is-ignored.js`.
//
//   node and native: 5
//   interpreter:     NaN
console.log(Number({ [Symbol.toPrimitive]: () => 5 }));
