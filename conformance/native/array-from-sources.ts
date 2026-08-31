// `Array.from` takes an iterable *or* an array-like, and which one is decided up front.
//
// The native backend routed every source through its iteration helper, so `Array.from({length:
// 2, 0: "x"})` -- `Array.from(arguments)`, `Array.from({length: n})`, `Array.from(nodeList)`, the
// shape half the array-likes in the wild have -- threw "is not iterable" where node and the
// interpreter build an array. The choice is made by whether the source has a callable
// `@@iterator`, not by trying to iterate and recovering, which is why the array-like walk cannot
// live in the shared helper: spread has no array-like form and must keep refusing.
console.log("string", JSON.stringify(Array.from("abc")), JSON.stringify([..."ab"]));
console.log("arraylike", JSON.stringify(Array.from({ length: 2, 0: "x", 1: "y" })));
console.log("arraylike-holes", JSON.stringify(Array.from({ length: 3 })));
console.log("arraylike-mapped", JSON.stringify(Array.from({ length: 3 }, (_, i) => i * 2)));
console.log("arraylike-bad-length", JSON.stringify(Array.from({ length: -1 })), JSON.stringify(Array.from({})));
console.log("iterable-wins", JSON.stringify(Array.from(new Set([1, 2]))), JSON.stringify(Array.from([3, 4])));
try { console.log("spread", JSON.stringify([...{ length: 1 }])); } catch (e) { console.log("spread-threw", e.constructor.name); }
