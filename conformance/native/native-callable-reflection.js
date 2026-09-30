const native = Object.getOwnPropertyDescriptor;
native.compatProbe = 7;
console.log(Object.keys(native).join(","));
console.log(Reflect.ownKeys(native).sort().join(","));
console.log(Object.getOwnPropertyDescriptor(native, "name").enumerable);

const expression = /x/;
console.log(expression.length === undefined, expression.hasOwnProperty("length"));
