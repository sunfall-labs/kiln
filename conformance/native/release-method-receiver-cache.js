// kiln-native: profile=native-release
function makeReceiver(label) {
  return {
    label,
    calls: 0,
    method(value) { this.calls++; return `${this.label}:${value}:${this.calls}`; },
    strictMethod() { 'use strict'; return this; },
    arrow: () => this,
  };
}

const lexicalReceiver = { lexical: true };
const receiver = makeReceiver.call(lexicalReceiver, 'receiver');
for (let i = 0; i < 4; i++) {
  if (receiver.method(i) !== `receiver:${i}:${i + 1}`) throw new Error('cached method lost receiver');
}
const bare = receiver.method;
if (!bare('bare').startsWith('undefined:bare:')) throw new Error('sloppy bare call receiver');
if (receiver.strictMethod() !== receiver) throw new Error('strict method receiver');
const strictBare = receiver.strictMethod;
if (strictBare() !== undefined) throw new Error('strict bare call receiver');
for (let i = 0; i < 4; i++) {
  if (receiver.arrow() !== lexicalReceiver) throw new Error('arrow receiver handling');
}
function sloppyReceiver() { return this; }
if (sloppyReceiver() !== globalThis || sloppyReceiver.call(null) !== globalThis || sloppyReceiver.call(undefined) !== globalThis) throw new Error('sloppy nullish receiver');
if (sloppyReceiver.call(receiver) !== receiver) throw new Error('sloppy explicit receiver');
const callbackThis = { total: 0 };
[1, 2, 3].forEach(function (value) { this.total += value; }, callbackThis);
if (callbackThis.total !== 6) throw new Error('callback thisArg was replaced');
if (JSON.stringify([1, 2, 3].map(function (value) { return value * this.factor; }, { factor: 2 })) !== '[2,4,6]') throw new Error('map callback thisArg');
if (JSON.stringify([1, 2, 3].filter(function (value) { return value >= this.min; }, { min: 2 })) !== '[2,3]') throw new Error('filter callback thisArg');
console.log('cached method receiver checks passed');
