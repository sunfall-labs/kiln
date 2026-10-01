const map = new Map();
map.present = undefined;
console.log(Object.prototype.hasOwnProperty.call(map, "present"));
console.log(Object.prototype.hasOwnProperty.call(map, "missing"));

const promise = Promise.resolve();
promise.status = "ready";
console.log(Object.prototype.hasOwnProperty.call(promise, "status"));
console.log(Object.prototype.hasOwnProperty.call(promise, "missing"));
