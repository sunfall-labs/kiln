console.log(typeof new Set()[Symbol.iterator], typeof new Map()[Symbol.iterator], typeof 'ab'[Symbol.iterator]);
console.log(typeof new Set()["@@iterator"], typeof new Map()["@@iterator"], typeof 'ab'["@@iterator"]);
function* g() { yield 1 }
console.log(typeof g()[Symbol.iterator]);
console.log(typeof g()["@@iterator"]);
