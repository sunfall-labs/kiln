// Native-only collector-stress rollback probe. The interpreter implements direct graph
// semantics with a plain wire, while this exercises native candidate-chunk provenance.
import { isolate, spawnActor, transfer } from 'kiln:actors';

const owner = await spawnActor({
  module: new URL('../../../conformance/native/support/actor-counter.mjs', import.meta.url),
  export: 'Counter',
  args: [1],
});

async function moveOrCheckStressRollback(factory, check, method = 'echo') {
  const ticket = transfer(isolate(factory));
  try {
    check(await owner.ref.call(method, [ticket]));
    return true;
  } catch (error) {
    const stressed = process.env.KILN_GC_STRESS === '1';
    if (!stressed || error.name !== 'DataCloneError' ||
        error.message !== 'graph transfer needs ownership proof') throw error;
    // Every-allocation collection may invalidate candidate provenance. A refusal
    // must return the graph and ticket to their original state for a retry.
    let retryRefused = false;
    try { await owner.ref.call(method, [ticket]); }
    catch (retryError) {
      retryRefused = retryError.name === 'DataCloneError' &&
        retryError.message === 'graph transfer needs ownership proof';
    }
    if (!retryRefused) throw new Error('refused graph ticket was consumed or moved on retry');
    return false;
  }
}

try {
  const moved = await moveOrCheckStressRollback(
    () => ({ answer: 42 }),
    answer => { if (answer.answer !== 42) throw new Error('direct graph move lost its value'); },
  );
  const prefix = 'a'.repeat(40000);
  const largeMoved = await moveOrCheckStressRollback(
    () => ({ text: prefix + prefix }),
    answer => {
      if (answer.text.length !== 80000 || answer.text[0] !== 'a' ||
          answer.text[79999] !== 'a') throw new Error('large direct string lost its bytes');
    },
  );
  let arraysMoved = true;
  for (const length of [128, 16384]) {
    const movedArray = await moveOrCheckStressRollback(
      () => {
        const values = [];
        for (let index = 0; index < length; ++index) values.push(index * 3);
        return values;
      },
      answer => {
        if (answer.length !== length || answer[0] !== 0 ||
            answer[127] !== 381 || answer[length - 1] !== (length - 1) * 3) {
          throw new Error('direct side-backed array lost its elements');
        }
      },
    );
    arraysMoved = arraysMoved && movedArray;
  }
  // Every-allocation collection invalidates the candidate before transfer; the small array
  // above checks that rollback. Keep the 100,000-element growth check on the positive path.
  const wideMoved = process.env.KILN_GC_STRESS === '1' ? false : await moveOrCheckStressRollback(
    () => {
      const values = [];
      for (let index = 0; index < 100000; ++index) values.push(3);
      return values;
    },
    answer => {
      if (!Array.isArray(answer) || answer.length !== 4 || answer[0] !== 200001 ||
          answer[1] !== 3 || answer[2] !== 3 || answer[3] !== 4) {
        throw new Error('receiver could not grow a direct side-backed array');
      }
    },
    'growTransferredArray',
  );
  arraysMoved = arraysMoved && wideMoved;

  const shapedMoved = await moveOrCheckStressRollback(
    () => {
      const object = {};
      object.s00 = 0; object.s01 = 1; object.s02 = 2; object.s03 = 3;
      object.s04 = 4; object.s05 = 5; object.s06 = 6; object.s07 = 7;
      object.s08 = 8; object.s09 = 9; object.s10 = 10; object.s11 = 11;
      return object;
    },
    answer => {
      if (!Array.isArray(answer) || answer.length !== 4 || answer[0] !== 17 ||
          answer[1] !== 0 || answer[2] !== 8 || answer[3] !== 16) {
        throw new Error('receiver could not grow direct side-backed shape');
      }
    },
    'growTransferredShape',
  );

  const dictionaryMoved = await moveOrCheckStressRollback(
    () => ({
      p00: 0, p01: 1, p02: 2, p03: 3, p04: 4, p05: 5, p06: 6, p07: 7,
      p08: 8, p09: 9, p10: 10, p11: 11, p12: 12, p13: 13, p14: 14, p15: 15,
      p16: 16, p17: 17, p18: 18, p19: 19, p20: 20, p21: 21, p22: 22, p23: 23,
      p24: 24, p25: 25, p26: 26, p27: 27, p28: 28, p29: 29, p30: 30, p31: 31,
      p32: 32, p33: 33, p34: 34, p35: 35, p36: 36, p37: 37, p38: 38, p39: 39,
    }),
    answer => {
      if (!Array.isArray(answer) || answer.length !== 7 || answer[0] !== 'p00' ||
          answer[1] !== 0 || answer[2] !== 'number' || answer[3] !== 0 ||
          answer[4] !== false || answer[5] !== true || answer[6] !== 41) {
        throw new Error('receiver could not read and grow a direct dictionary');
      }
    },
    'inspectPreparedDictionary',
  );

  let alias;
  const retained = isolate(() => {
    alias = { answer: 17 };
    return alias;
  });
  const ticket = transfer(retained);
  let refused = false;
  try {
    await owner.ref.call('echo', [ticket]);
  } catch (error) {
    refused = error.name === 'DataCloneError' &&
      error.message === 'graph transfer needs ownership proof';
  }
  if (!refused || alias.answer !== 17) {
    throw new Error('retained sender alias was moved');
  }
  let retainedArray;
  const retainedArrayTicket = transfer(isolate(() => {
    retainedArray = [];
    for (let index = 0; index < 128; ++index) retainedArray.push(index);
    return retainedArray;
  }));
  let arrayRefused = false;
  try { await owner.ref.call('echo', [retainedArrayTicket]); }
  catch (error) {
    arrayRefused = error.name === 'DataCloneError' &&
      error.message === 'graph transfer needs ownership proof';
  }
  if (!arrayRefused || retainedArray.length !== 128 || retainedArray[127] !== 127) {
    throw new Error('retained sender array or its side buffer was moved');
  }
  console.log(moved && largeMoved && arraysMoved && shapedMoved && dictionaryMoved ?
    'direct moves succeed; retained alias refuses' :
    'stress refusal rolls back; retained alias refuses');
} finally {
  await owner.stop();
}
