function makeReceiver(label) {
  return {
    label,
    calls: 0,
    method(value) { this.calls++; return `${this.label}:${value}:${this.calls}`; },
    strictMethod() { 'use strict'; return this; },
    arrow: () => this,
  };
}

const receiver = makeReceiver('receiver');
for (let i = 0; i < 4; i++) {
  if (receiver.method(i) !== `receiver:${i}:${i + 1}`) throw new Error('cached method lost receiver');
}
const bare = receiver.method;
if (!bare('bare').startsWith('undefined:bare:')) throw new Error('sloppy bare call receiver');
if (receiver.strictMethod() !== receiver) throw new Error('strict method receiver');
const strictBare = receiver.strictMethod;
if (strictBare() !== undefined) throw new Error('strict bare call receiver');
if (receiver.arrow() !== receiver.arrow()) throw new Error('arrow receiver handling');
const callbackThis = { total: 0 };
[1, 2, 3].forEach(function (value) { this.total += value; }, callbackThis);
if (callbackThis.total !== 6) throw new Error('callback thisArg was replaced');
console.log('cached method receiver checks passed');
