// JavaScript switches to exponential notation outside 1e-7 < |n| < 1e21 and Rust does not, so
// `String(1e21)` is not `1e21.to_string()`. Both boundaries are exercised from both sides.
console.log(String(1e20), String(1e21), String(1e-6), String(1e-7));
console.log(String(1.5e21), String(1.2345e-7), String(-1e21), String(5e-324));
console.log(String(0.1 + 0.2), String(1 / 3), String(100), String(1.5));
console.log(String(-0), String(0), String(0 / 0), String(1 / 0), String(-1 / 0));
console.log(String(123456789012345678901), String(2 ** 53), String(0.000001));
// A console shows -0 as itself; String() does not. They are different questions.
console.log(-0, 0, [-0]);
