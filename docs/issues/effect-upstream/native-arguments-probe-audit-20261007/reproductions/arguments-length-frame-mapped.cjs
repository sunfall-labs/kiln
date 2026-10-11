// These simple-parameter CommonJS functions are compiled as sloppy wrapper code. Keep the
// argc-only callee adjacent to both mapped-parent cases: the calls must not suppress the
// caller's parameter/arguments aliasing, whether its arguments object is still lazy or has
// already been materialized.
function argcOnly() {
  return arguments.length;
}

function lazyParent(value) {
  const actualCount = argcOnly(1, 2, 3);
  value = 'lazy-after-parameter-write';
  const afterParameterWrite = arguments[0];
  arguments[0] = 'lazy-after-arguments-write';
  return [actualCount, afterParameterWrite, value, arguments[0]];
}

function materializedParent(value) {
  const before = arguments[0];
  const actualCount = argcOnly();
  value = 'materialized-after-parameter-write';
  const afterParameterWrite = arguments[0];
  arguments[0] = 'materialized-after-arguments-write';
  return [actualCount, before, afterParameterWrite, value, arguments[0]];
}

console.log(JSON.stringify({
  lazy: lazyParent('lazy-before'),
  materialized: materializedParent('materialized-before')
}));
