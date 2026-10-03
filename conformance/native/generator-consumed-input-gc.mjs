import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';

setFlagsFromString('--expose_gc');
const collect = runInNewContext('gc');
setFlagsFromString('--no-expose_gc');

function makeControl() { return new WeakRef({}); }
const control = makeControl();
const refs = [];
const create = () => {
  const target = {};
  refs.push(new WeakRef(target));
  return target;
};
function consume(value) { console.log(value === refs[0].deref()); }
function* values() {
  consume(yield 0);
  yield 1;
  yield 2;
}
const generator = values();
function advance() {
  generator.next();
  generator.next(create());
}
advance();

async function finish() {
  // Collection crosses jobs so WeakRef's KeepDuringJob roots have expired. Do not
  // await this function at module scope: that would retain the initial module job.
  for (let i = 0; i < 8; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    collect();
  }
  console.log(control.deref() === undefined, refs[0].deref() === undefined,
    generator.next().value);
}
finish().catch(console.error);
