import { Actor } from "kiln:actors";

// Keeps what its constructor was given, for a caller to inspect.
export class Initial extends Actor {
  constructor(...args) {
    super();
    this.args = args;
  }
  describe() {
    return this.args.map((value) => value instanceof ArrayBuffer
      ? `buffer:${new Uint8Array(value)[0]}` : JSON.stringify(value));
  }
}
export class Throws extends Actor {
  constructor() { super(); throw new RangeError("constructor refused"); }
}
// Not an actor: it only carries the registered symbol the actor brand once was.
export class Forged {
  describe() { return ["forged"]; }
}
Object.defineProperty(Forged.prototype, Symbol.for("kiln.actor.base"), { value: true });
