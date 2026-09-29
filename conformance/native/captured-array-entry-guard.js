// kiln-native: profile=native-release
// A sole initialization after an effectful prefix permits an array entry guard,
// but cannot permit unconditional elimination of the captured binding's TDZ check.
function makeReader(beforeInitialization) {
  function read(useArray) {
    if (!useArray) return 0;
    let sum = 0;
    for (let i = 0; i < values.length; i++) sum += values[i];
    return sum;
  }
  beforeInitialization(read);
  const values = [1, 2, 3];
  return read;
}
const reader = makeReader((read) => {
  console.log("unused-before-init", read(false));
  try {
    read(true);
    console.log("missing-tdz");
  } catch (error) {
    console.log("before-init", error.name);
  }
});
console.log("initialized", reader(true), reader(true));

// A later write must prevent caching the original array for the whole call.
function mutableReader() {
  let values = [1, 2];
  return function read() {
    const first = values.length;
    values = [3, 4, 5];
    return first + values.length;
  };
}
const mutable = mutableReader();
console.log("mutable", mutable(), mutable());

// A loop may execute a single syntactic store repeatedly into the same cell.
function loopReader() {
  let values;
  function read() { return values.length; }
  let sum = 0;
  for (let i = 0; i < 3; i++) {
    values = [i];
    sum += read();
  }
  return sum;
}
console.log("loop", loopReader());

// Parameters have an implicit initialization before their first explicit store.
// Calling back into the reader before that store must not freeze the argument.
function* parameterReader(values) {
  function read(assign) {
    const first = values.length;
    assign();
    return first + values.length;
  }
  yield read;
  values = [4, 5, 6];
  yield read;
}
const iterator = parameterReader([1]);
const parameter = iterator.next().value;
console.log("parameter", parameter(() => iterator.next()), parameter(() => {}));
