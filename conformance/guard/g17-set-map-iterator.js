console.log(typeof new Set()[Symbol.iterator], typeof new Map()[Symbol.iterator], typeof 'ab'[Symbol.iterator]);
function* g() { yield 1 }
console.log(typeof g()[Symbol.iterator]);
