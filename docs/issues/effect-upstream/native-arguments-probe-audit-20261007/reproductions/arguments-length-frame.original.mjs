// Arguments.length is the actual call argc only while the exotic Arguments object is
// unobservable. Keep explicit object-observation controls beside the eligible reads so
// argc lowering cannot silently change aliases, descriptors, closures, or generator state.
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

function* generatorReadsArguments() {
  yield arguments.length;
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
// Function-constructor code is sloppy. The parent has mapped arguments; calling an
// argc-only child must not make the parent lose parameter/arguments aliasing.
const invokeFromSloppyMappedParents = Function('child',
  'return {' +
  '  lazy: function parent(value) {' +
  '    const childLength = child();' +
  '    value = "lazy-after";' +
  '    const afterParameterWrite = arguments[0];' +
  '    arguments[0] = "lazy-from-arguments";' +
  '    return [childLength, afterParameterWrite, value, arguments[0]];' +
  '  }("lazy-before"),' +
  '  materialized: function parent(value) {' +
  '    const before = arguments[0];' +
  '    const childLength = child(1, 2, 3);' +
  '    value = "materialized-after";' +
  '    const afterParameterWrite = arguments[0];' +
  '    arguments[0] = "materialized-from-arguments";' +
  '    return [childLength, before, afterParameterWrite, value, arguments[0]];' +
  '  }("materialized-before")' +
'};');
console.log(JSON.stringify({
  zero: zeroActuals(),
  several: severalActuals(1, 2, 3),
  wrongArityBoxedCall: oneFormalActualLength(1, 2, 3),
  allocation: lengthWithAllocation('x', 'y'),
  alias: aliasAndWriteLength('original', 'second'),
  escapedArrow: escapedArrow(),
  directEval: directEvalReadsArguments(1, 2),
  generator: Array.from(generatorReadsArguments(1, 2, 3)),
  async: await asyncReadsArguments(1, 2),
  ordinaryCall: newTargetAndArguments(1),
  constructorCall: new newTargetAndArguments(1, 2),
  thrown: throwsAfterReadingArguments('a', 'b'),
  sloppyMappedParents: invokeFromSloppyMappedParents(zeroActuals)
}));
