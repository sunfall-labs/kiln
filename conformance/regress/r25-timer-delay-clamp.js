// Node's timer delay: anything but 1 to 2^31 - 1 milliseconds is 1 ms, so a zero, negative, NaN,
// fractional or too-large delay runs in order with a 1 ms timer armed before it -- and after it
// when armed after -- rather than ahead of it. (NaN, negative and overflowing delays also warn
// on stderr, as `TimeoutNaNWarning`, `TimeoutNegativeWarning` and `TimeoutOverflowWarning`.)
setTimeout(() => console.log("one ms"), 1);
setTimeout(() => console.log("zero"), 0);
setTimeout(() => console.log("negative"), -5);
setTimeout(() => console.log("nan"), "soon");
setTimeout(() => console.log("no delay"));
setTimeout(() => console.log("half"), 0.5);
setTimeout(() => console.log("two ms"), 2);
setTimeout(() => console.log("overflow"), 2 ** 31);
setTimeout(() => console.log("one ms again"), 1);
const interval = setInterval(() => {
  console.log("interval at zero");
  clearInterval(interval);
}, 0);
setTimeout(() => console.log("three ms"), 3);
