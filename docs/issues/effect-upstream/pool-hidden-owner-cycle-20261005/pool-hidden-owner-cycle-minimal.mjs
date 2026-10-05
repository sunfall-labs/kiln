import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';
setFlagsFromString('--expose_gc');
const collect = runInNewContext('gc');
setFlagsFromString('--no-expose_gc');
function plainCycle() {
  const resource = {};
  const owner = { resource };
  owner.self = owner;
  return new WeakRef(resource);
}
function arrayCycle() {
  const resource = {};
  const owner = [resource];
  owner.push(owner);
  return new WeakRef(resource);
}
function mixedCycle() {
  const resource = {};
  const owner = { entries: [[resource]] };
  owner.entries.push(owner);
  return new WeakRef(resource);
}
function control() { return new WeakRef({}); }
const refs = [plainCycle(), arrayCycle(), mixedCycle(), control()];
const live = { resource: {} };
live.self = live;
const liveRef = new WeakRef(live.resource);
async function finish() {
  for (let i = 0; i < 8; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    collect();
  }
  console.log(refs.map(ref => ref.deref() === undefined).join(','));
  console.log(liveRef.deref() === live.resource);
}
finish().catch(console.error);
