// kiln eval smoke test (authored for kiln, not from porffor).
const { createMachine, createActor } = require('xstate');

const toggle = createMachine({
  initial: 'inactive',
  states: {
    inactive: { on: { TOGGLE: 'active' } },
    active: { on: { TOGGLE: 'inactive' } },
  },
});

const actor = createActor(toggle);
actor.start();
console.log(actor.getSnapshot().value);
actor.send({ type: 'TOGGLE' });
console.log(actor.getSnapshot().value);
actor.send({ type: 'TOGGLE' });
console.log(actor.getSnapshot().value);
actor.stop();

const counter = createMachine({
  context: { count: 0 },
  on: {
    INC: { actions: ({ context }) => ({ count: context.count + 1 }) },
  },
});
const counterActor = createActor(counter).start();
counterActor.send({ type: 'INC' });
counterActor.send({ type: 'INC' });
console.log(counterActor.getSnapshot().context.count);
counterActor.stop();
