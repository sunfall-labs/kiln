const mapIteratorPrototype = Object.getPrototypeOf(new Map().keys());
const anotherMapIteratorPrototype = Object.getPrototypeOf(new Map().values());
const setIteratorPrototype = Object.getPrototypeOf(new Set().values());

console.log(mapIteratorPrototype !== Object.getPrototypeOf({}));
console.log(mapIteratorPrototype === anotherMapIteratorPrototype);
console.log(mapIteratorPrototype !== setIteratorPrototype);
console.log(typeof mapIteratorPrototype.next, typeof setIteratorPrototype.next);
console.log(Object.prototype.toString.call(new Map().keys()));
console.log(Object.prototype.toString.call(new Set().values()));
