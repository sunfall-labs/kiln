// TypeScript parameter properties declare and assign fields from the constructor's parameters.
// They were once dropped without a word: `constructor(public fn)` left `this.fn` undefined and the
// program failed far away ("object.fn is not a function"). This probe checks its own answers, so
// both engines agreeing on a wrong value still fails.
const failures: string[] = [];
function check(what: string, actual: unknown, expected: unknown) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  console.log(`${ok ? "ok" : "FAIL"} ${what}: ${JSON.stringify(actual)}`);
  if (!ok) failures.push(what);
}

class Point {
  constructor(public x: number, private readonly y = 2, protected z?: number, plain = 4) {
    void plain;
  }
  sum(): number {
    return this.x + this.y + (this.z ?? 0);
  }
}
const p = new Point(1);
check("modifiers assign, defaults apply, plain parameters do not", Object.keys(p), ["x", "y", "z"]);
check("methods read them", p.sum(), 3);

// Field initializers run before the constructor body, as TypeScript emits for ES2022 targets.
class WithField {
  seen: string;
  constructor(public name: string) {
    this.seen = `${this.name}!`;
  }
}
check("body sees the assignment", new WithField("a").seen, "a!");

// In a derived class the assignments follow super(), where `this` begins to exist.
class Base {
  constructor(public kind: string) {}
}
class Derived extends Base {
  readonly label: string;
  constructor(override kind: string, public extra: number) {
    super(kind.toUpperCase());
    this.label = `${this.kind}:${this.extra}`;
  }
}
const d = new Derived("leaf", 7);
check("derived class: after super, overriding the base's value", [d.kind, d.extra, d.label], ["leaf", 7, "leaf:7"]);

if (failures.length > 0) throw new Error(`parameter properties: ${failures.join(", ")}`);
