function indexed(a) { return [a[0], a[1], a.length]; }
function getter(a) { return a[0]; }
function ownAccessor() {
  const a = [11, 7];
  Object.defineProperty(a, "0", {
    configurable: true,
    enumerable: true,
    get() { return 11; },
  });
  return a;
}
function inheritedAccessor() {
  const previous = Object.getOwnPropertyDescriptor(Array.prototype, "1");
  Object.defineProperty(Array.prototype, "1", {
    configurable: true,
    get() { return 12; },
  });
  try { return indexed(new Array(2)); }
  finally {
    if (previous) Object.defineProperty(Array.prototype, "1", previous);
    else delete Array.prototype[1];
  }
}
const dense = [4, 7];
const holes = [4, , 9];
const arraylike = {0: 4, 1: 7, length: 2};
const strings = "xy";
const outputs = {
  dense: indexed(dense),
  holes: indexed(holes),
  arraylike: indexed(arraylike),
  strings: indexed(strings),
  ownAccessor: indexed(ownAccessor()),
  inheritedAccessor: inheritedAccessor(),
};
let changing;
try { changing = [indexed([1, 2]), indexed({0: 3, 1: 4, length: 2})]; }
catch (error) { changing = String(error); }
let reassigned;
function reassignedParam(a) { a = {0: 8, length: 1}; return [a[0], a.length]; }
reassigned = reassignedParam([1, 2]);
console.log(JSON.stringify({outputs, changing, reassigned}));
