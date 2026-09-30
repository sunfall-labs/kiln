// kiln eval smoke test (authored for kiln, not from porffor).
const { ok, err } = require('neverthrow');
console.log(ok(1).map((x) => x + 1)._unsafeUnwrap());
console.log(err('boom').mapErr((e) => e + '!')._unsafeUnwrapErr());
console.log(ok(2).andThen((x) => ok(x * 3))._unsafeUnwrap());
console.log(err('x').isErr(), ok(1).isOk());
console.log(ok(1).andThen((a) => ok(2).map((b) => a + b))._unsafeUnwrap());
console.log(err('e').orElse((e) => ok('recovered:' + e))._unsafeUnwrap());
