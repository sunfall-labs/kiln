// A throw travels outward until a handler takes it. In the generated code that is a status
// returned from every call and a per-body handler stack, because C has no unwinder -- and the
// catch target is a switch rather than a jump, because C has no computed goto either.
function risky(n: number): number {
  if (n < 0) { throw "negative"; }
  return n * 2;
}
try {
  console.log(risky(3));
  console.log(risky(-1));
  console.log("unreachable");
} catch (e) {
  console.log("caught", e);
}

// A throw of anything, not just a string.
try { throw { code: 42, why: "because" }; } catch (e) { console.log(e); }
try { throw [1, 2]; } catch (e) { console.log(e); }
try { throw 7; } catch (e) { console.log(e, typeof e); }

// Caught inside the function that threw, so nothing crosses a frame.
function handled(): string {
  try { throw "inner"; } catch (e) { return `handled ${e}`; }
}
console.log(handled());

// Nested handlers: the innermost takes it, and the outer one is still armed afterwards.
try {
  try { throw "first"; } catch (e) { console.log("inner sees", e); }
  throw "second";
} catch (e) {
  console.log("outer sees", e);
}

// Rethrowing from a catch, across two frames.
function rethrow(): void {
  try { throw "original"; } catch (e) { throw `wrapped ${e}`; }
}
try { rethrow(); } catch (e) { console.log(e); }

// A throw from deep in a call chain unwinds every frame between.
function level3(): number { throw "deep"; }
function level2(): number { return level3(); }
function level1(): number { return level2(); }
try { level1(); } catch (e) { console.log("from depth", e); }

// `finally` runs on both paths, and after a `return`.
function withFinally(fail: boolean): string {
  let log = "";
  try {
    if (fail) { throw "x"; }
    log = `${log}ok,`;
  } catch (e) {
    log = `${log}caught,`;
  } finally {
    log = `${log}finally`;
  }
  return log;
}
console.log(withFinally(false), withFinally(true));

// A throw out of a callback the runtime is driving, which unwinds through the runtime's own loop.
try {
  [1, 2, 3].map((x: number) => { if (x === 2) { throw "in map"; } return x; });
} catch (e) {
  console.log("callback", e);
}

// The stack is restored to where the `try` began, however deep the expression that threw got.
function depth(): number {
  try { return 1 + risky(-1) + 100; } catch (e) { return 5; }
}
console.log(depth());

// `finally` runs when the `catch` block itself throws.
function nestedThrowInCatch(): string {
  try {
    try {
      throw 1;
    } catch (e: any) {
      throw e + 1;
    } finally {
      console.log("inner-finally");
    }
  } catch (e: any) {
    return `outer:${e}`;
  }
}
console.log(nestedThrowInCatch());

