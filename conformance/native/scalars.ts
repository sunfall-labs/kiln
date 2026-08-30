// The surface the native backend had before functions, kept here so the closure work cannot
// quietly regress it.
let total = 0;
while (total < 5) { total += 1; }
console.log(total, total * 3, total / 2, total % 3, total ** 2, -total, +total, ~total);
console.log(total > 4, total <= 5, total === 5, total !== 5, total == "5", total != "5");
console.log("a" + "b", "n=" + total, `t=${total}`, "abc" < "abd", "Z" < "a");
console.log(1 & 3, 1 | 6, 5 ^ 3, 1 << 4, -16 >> 2, -16 >>> 28);
console.log(typeof total, typeof "s", typeof true, typeof undefined, typeof null);
console.log(0.1 + 0.2, 1 / 3, 1e21, 1e-7, -0, 1 / 0, -1 / 0, 0 / 0);
const flag = total === 5 ? "yes" : "no";
console.log(flag, !flag, null ?? "fallback", undefined ?? total, flag || "or", flag && "and");
