// `flatMap` flattens each answer that is an array through FlattenIntoArray, which asks
// HasProperty for every index and skips a hole -- the target index advances only past an
// element it wrote. The interpreter appended each hole as a gap (read back as undefined).
const shared = [1, , 3];
console.log(JSON.stringify([10, 20, 30, 40].flatMap((x, i) => (i & 1) ? shared : [x])));
const leading = [1, 2].flatMap(() => [, 5, , 6]);
console.log(leading.length, JSON.stringify(leading), 0 in leading, 3 in leading);
const holeyReceiver = [1, , 3].flatMap((x) => [x, , x]);
console.log(holeyReceiver.length, JSON.stringify(holeyReceiver));
const arrayLike = Array.prototype.flatMap.call({ length: 2, 0: "a", 1: "b" }, (x) => [x, , x]);
console.log(arrayLike.length, JSON.stringify(arrayLike));
const sparse = [];
sparse[5] = "end";
const long = ["start"].flatMap(() => sparse);
console.log(long.length, JSON.stringify(long));
console.log(JSON.stringify([[1, , 2], [, 3]].flat()), JSON.stringify([1, [, 2, [3, , 4]]].flat(Infinity)));
