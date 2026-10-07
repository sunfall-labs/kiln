import { setFlagsFromString } from 'node:v8';
import { runInNewContext } from 'node:vm';

setFlagsFromString('--expose_gc');
const collect = runInNewContext('gc');
setFlagsFromString('--no-expose_gc');

const references = [];
let escaped;
let finallyResult;
let throwResult;
let evalResult;

function* capturedAcrossYield() {
  const value = { marker: 'captured' };
  references.push(new WeakRef(value));
  escaped = () => value.marker;
  yield 0;
  yield 1;
}

function* returnedThroughFinally() {
  const value = { marker: 'return-finally' };
  try {
    yield 0;
  } finally {
    finallyResult = value.marker;
  }
}

function* thrownThroughFinally() {
  const value = { marker: 'throw-finally' };
  try {
    yield 0;
  } finally {
    throwResult = value.marker;
  }
}

function* readWithDirectEval() {
  const value = { marker: 'direct-eval' };
  yield 0;
  evalResult = eval('value.marker');
}

function* delegatedResult() {
  function* child() {
    return { marker: 'delegated' };
  }
  const result = yield* child();
  yield 0;
  return result.marker;
}

const captured = capturedAcrossYield();
captured.next();
captured.next();

const returned = returnedThroughFinally();
returned.next();

const thrown = thrownThroughFinally();
thrown.next();

const evaluated = readWithDirectEval();
evaluated.next();

const delegated = delegatedResult();
delegated.next();

async function finish() {
  for (let i = 0; i < 8; i++) {
    await new Promise(resolve => setTimeout(resolve, 0));
    collect();
  }
  const closureValue = escaped();
  returned.return();
  try {
    thrown.throw(new Error('resume throw'));
  } catch {}
  evaluated.next();
  const delegatedValue = delegated.next().value;
  console.log(JSON.stringify({
    captured: closureValue,
    capturedAlive: references[0].deref() !== undefined,
    finallyResult,
    throwResult,
    evalResult,
    delegatedValue
  }));
}

finish().catch(console.error);
