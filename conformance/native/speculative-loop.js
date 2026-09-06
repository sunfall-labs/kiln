// A versioned body guesses beyond its parameters: an upvalue stored once and indexed or
// measured is an array, checked at entry; a method call whose answer feeds arithmetic answers
// a number, checked where it answers, and a call that answers anything else leaves for the
// generic clone right after that call. Every guess here is wrong somewhere.
const nums = [1, 2, 3, , 5];            // a hole: the element tier misses, the read answers undefined
const str = "hello";                    // not an array: the entry guard fails, the generic clone runs
const like = { length: 2, 0: "a", 1: "b" };
class V { constructor(v) { this.v = v; } val() { return this.v; } }
const objs = [new V(1), new V(2), new V("x"), new V(3), new V(null), new V(4.5)];
function sumNums() { let s = 0; for (let i = 0; i < nums.length; i++) s += nums[i]; return s; }
function joinStr() { let s = ""; for (let i = 0; i < str.length; i++) s += str[i] + "."; return s; }
function joinLike() { let s = ""; for (let i = 0; i < like.length; i++) s += like[i]; return s; }
function sumVals() { let s = 0; for (let i = 0; i < objs.length; i++) s += objs[i].val(); return s; }
function sumTwice() { let s = 0; for (let r = 0; r < 2; r++) for (let i = 0; i < objs.length; i++) { s += objs[i].val(); s *= 1; } return s; }
function grow() { let s = 0; for (let i = 0; i < nums.length; i++) { s += nums[i] || 0; if (nums.length < 8) nums.push(i * 10); } return s + "/" + nums.length; }
function outOfRange() { let s = 0; for (let i = -1; i <= nums.length; i++) s += nums[i] === undefined ? 100 : nums[i]; return s; }
console.log([sumNums(), joinStr(), joinLike(), sumVals(), sumTwice(), grow(), outOfRange()].join("|"));
