// Node's timer delay: a zero, fractional or missing delay is 1 ms, so it runs in order with a
// 1 ms timer armed before it rather than ahead of it. (The delays that also warn on stderr are
// in conformance/regress/r25, where stderr is not part of the comparison.)
setTimeout(() => console.log("one ms"), 1);
setTimeout(() => console.log("zero"), 0);
setTimeout(() => console.log("no delay"));
setTimeout(() => console.log("half"), 0.5);
setTimeout(() => console.log("two ms"), 2);
setTimeout(() => console.log("one ms again"), 1);
const interval = setInterval(() => {
  console.log("interval at zero");
  clearInterval(interval);
}, 0);
