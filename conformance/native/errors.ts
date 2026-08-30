// An error is an ordinary object carrying `message`, `name` and `stack`, none of them enumerated:
// `err.message` reads and `Object.keys(err)` is empty, which is why `JSON.stringify(err)` is `{}`.
const e = new Error("boom");
console.log(e.message, e.name, String(e), e instanceof Error, typeof e);
console.log(Object.keys(e), JSON.stringify(e), JSON.stringify({ wrapped: e }), { ...e });

const t = new TypeError("bad type");
console.log(t.message, t.name, String(t), t instanceof TypeError, t instanceof Error);
console.log(new RangeError("r").name, new ReferenceError("q").name, new SyntaxError("s").name);
console.log(new Error().message, String(new Error()), new Error("x") instanceof TypeError);

// A thrown error carries its message across frames.
function fails(): number { throw new TypeError("from a frame"); }
try { fails(); } catch (err: any) { console.log(err.name, err.message, err instanceof TypeError); }

// A program may add fields, and those *are* enumerated -- which is how a structured error
// survives `JSON.stringify` while the built-in ones stay out of it.
const detailed: any = new Error("failed");
detailed.code = 42;
console.log(Object.keys(detailed), JSON.stringify(detailed), detailed.message, detailed.code);

// The payoff: an engine-level TypeError is a value the program can catch, not a refusal.
try { (undefined as any).x; } catch (err: any) { console.log("read", err instanceof TypeError, err.name); }
try { (null as any).y; } catch (err: any) { console.log("null", err instanceof TypeError); }
try { (1 as any)(); } catch (err: any) { console.log("call", err instanceof TypeError, err.name); }

// Unbounded recursion is a RangeError a program can catch, which is what lets a parser report a
// nesting limit rather than dying.
function forever(n: number): number { return forever(n + 1); }
try { forever(0); } catch (err: any) { console.log("depth", err instanceof RangeError); }

// And the distinction that makes all of this worth having: a catch takes what the program threw
// *and* what the language threw, and nothing else.
let caught = 0;
for (const attempt of [() => { throw new Error("mine"); }, () => (undefined as any).z]) {
  try { attempt(); } catch { caught += 1; }
}
console.log(caught);
