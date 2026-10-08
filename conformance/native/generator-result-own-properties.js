const previousValue = Object.getOwnPropertyDescriptor(Object.prototype, "value");
const previousDone = Object.getOwnPropertyDescriptor(Object.prototype, "done");
let inheritedGets = 0;
let inheritedSets = 0;
let syncNext;
let syncReturn;
let asyncNext;
let asyncReturn;

function install(key) {
  const descriptor = Object.create(null);
  descriptor.get = function () { inheritedGets++; throw new Error(`inherited ${key} getter`); };
  descriptor.set = function () { inheritedSets++; throw new Error(`inherited ${key} setter`); };
  descriptor.enumerable = true;
  descriptor.configurable = true;
  Object.defineProperty(Object.prototype, key, descriptor);
}

try {
  install("value");
  install("done");

  const sync = (function* () { yield 11; })();
  syncNext = sync.next();
  syncReturn = sync.return(12);

  const async = (async function* () { yield 21; })();
  asyncNext = await async.next();
  asyncReturn = await async.return(22);
} finally {
  if (previousValue) Object.defineProperty(Object.prototype, "value", previousValue);
  else delete Object.prototype.value;
  if (previousDone) Object.defineProperty(Object.prototype, "done", previousDone);
  else delete Object.prototype.done;
}

function checkStep(name, step, expectedValue, expectedDone) {
  const value = Object.getOwnPropertyDescriptor(step, "value");
  const done = Object.getOwnPropertyDescriptor(step, "done");
  if (!value || !done || value.value !== expectedValue || done.value !== expectedDone ||
      value.writable !== true || value.enumerable !== true || value.configurable !== true ||
      done.writable !== true || done.enumerable !== true || done.configurable !== true) {
    throw new Error(`${name} result descriptors or values differ`);
  }
  console.log(name, value.value, done.value, value.writable, value.enumerable, value.configurable,
              done.writable, done.enumerable, done.configurable);
}

checkStep("sync-next", syncNext, 11, false);
checkStep("sync-return", syncReturn, 12, true);
checkStep("async-next", asyncNext, 21, false);
checkStep("async-return", asyncReturn, 22, true);
if (inheritedGets !== 0 || inheritedSets !== 0) {
  throw new Error(`inherited accessors ran: gets=${inheritedGets} sets=${inheritedSets}`);
}
console.log("inherited", inheritedGets, inheritedSets);
