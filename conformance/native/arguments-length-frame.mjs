// Arguments.length is the actual call argc only while the exotic Arguments object is
// unobservable. Keep explicit object-observation controls beside the eligible reads so
// argc lowering cannot silently change aliases, descriptors, or closures.
function zeroActuals() {
  return arguments.length;
}

function severalActuals() {
  return arguments.length;
}

function oneFormalActualLength(value) {
  return arguments.length;
}

function lengthWithAllocation() {
  const length = arguments.length;
  const rooted = { length };
  return [rooted.length, new Error('after argc read').message];
}

function retainsObjectAcrossAllocation(value) {
  const actualCount = arguments.length;
  const allocated = { marker: 1, nested: [1, 2, 3] };
  return [actualCount, value.payload, allocated.nested.length];
}

function aliasAndWriteLength(first) {
  const alias = arguments;
  const before = alias.length;
  alias.length = 9;
  alias[0] = 'changed';
  return [before, arguments.length, arguments[0],
    Object.getOwnPropertyDescriptor(alias, 'length')];
}

function nestedArrowCapturesArguments(first) {
  return () => arguments.length;
}

function directEvalReadsArguments() {
  const beforeEval = arguments.length;
  return [beforeEval, eval('arguments.length')];
}

async function asyncReadsArguments() {
  return arguments.length;
}

function newTargetAndArguments() {
  return [arguments.length, new.target === newTargetAndArguments];
}

function throwsAfterReadingArguments() {
  const length = arguments.length;
  try {
    throw new Error('expected');
  } catch (error) {
    return [length, error.message];
  }
}

const escapedArrow = nestedArrowCapturesArguments('a', 'b', 'c');
console.log(JSON.stringify({
  zero: zeroActuals(),
  several: severalActuals(1, 2, 3),
  wrongArityBoxedCall: oneFormalActualLength(1, 2, 3),
  allocation: lengthWithAllocation('x', 'y'),
  retainedObject: retainsObjectAcrossAllocation({ payload: 'still rooted' }, 'extra-1', 'extra-2'),
  alias: aliasAndWriteLength('original', 'second'),
  escapedArrow: escapedArrow(),
  directEval: directEvalReadsArguments(1, 2),
  async: await asyncReadsArguments(1, 2),
  ordinaryCall: newTargetAndArguments(1),
  constructorCall: new newTargetAndArguments(1, 2),
  thrown: throwsAfterReadingArguments('a', 'b')
}));
