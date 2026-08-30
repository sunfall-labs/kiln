// `new.target` is ordinary-call undefined, the constructor named by `new`, and inherited by
// `super()`. Arrows capture it lexically; ordinary nested functions and methods reset it.
function ordinary(): boolean {
  return new.target === undefined;
}

function Constructed() {
  this.direct = new.target === Constructed;
  function nested(): boolean {
    return new.target === undefined;
  }
  this.nested = nested();
  this.arrow = (() => new.target === Constructed)();
}

class Base {
  constructor() {
    this.fromSuper = new.target === Derived;
  }
}

class Derived extends Base {}

class WithMethod {
  method(): boolean {
    return new.target === undefined;
  }
}

function* generator(): Generator<boolean> {
  yield new.target === undefined;
}

async function asyncTarget(): Promise<boolean> {
  return new.target === undefined;
}

const instance = new Constructed();
const derived = new Derived();
console.log(
  ordinary(),
  instance.direct,
  instance.nested,
  instance.arrow,
  derived.fromSuper,
  new WithMethod().method(),
  generator().next().value,
);
asyncTarget().then((value: boolean) => console.log(value));
