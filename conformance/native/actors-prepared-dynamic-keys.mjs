import { prepareTransfer, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('./support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

const key = 'dynamic-' + String(37);
const source = Object.create(null);
source[key] = 19;
if (source[key] !== 19) throw new Error('source assignment failed');
const prepared = prepareTransfer(source);
source[key] = 7;
const result = await owner.ref.call('inspectPreparedDictionary', [transfer(prepared)]);
if (result.join('|') !== `${key}|19|number|19|false|true|2` || source[key] !== 7 ||
    Object.getPrototypeOf(source) !== null) {
  throw new Error('dynamic-key snapshot lost its key, clone prototype or receiver ownership: ' +
                  result.join('|') + ' source=' + source[key]);
}

const ordinary = {};
ordinary['computed-' + String(41)] = { answer: 42 };
const moved = await owner.ref.call('echo', [transfer(prepareTransfer(ordinary))]);
if (moved['computed-41'].answer !== 42) {
  throw new Error('computed shaped key was not transferred');
}

const numeric = { label: { answer: 11 } };
numeric[127] = { answer: 3 };
numeric[0] = 1;
const numericHandle = prepareTransfer(numeric);
numeric[0] = 99;
numeric[127].answer = 8;
numeric.label.answer = 7;
const numericResult = await owner.ref.call('inspectPreparedNumericObject', [transfer(numericHandle)]);
if (numericResult.join('|') !== '0,127,label|1|3|11|4|0,127,255,label' ||
    numeric[0] !== 99 || numeric[127].answer !== 8 || numeric.label.answer !== 7) {
  throw new Error('prepared numeric properties lost order, snapshot or receiver growth: ' +
                  numericResult.join('|'));
}

await owner.stop();
console.log('prepared dynamic-key graphs move');
