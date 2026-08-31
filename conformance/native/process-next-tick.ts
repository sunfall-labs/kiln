const order: string[] = [];

const nextTick = process.nextTick;
order.push(`${delete process.nextTick}:${process.nextTick === undefined}`);
nextTick(function (first: string, second: number) {
  order.push(`${first}:${second}:${this === undefined}`);
  nextTick(function () { order.push("nested"); });
  queueMicrotask(function () { order.push("micro-from-tick"); });
}, "a", 2);
queueMicrotask(function () { order.push("micro"); });
setTimeout(function () { console.log(order.join(",")); }, 0);
