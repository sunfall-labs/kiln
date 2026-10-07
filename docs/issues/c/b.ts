console.log("toString", Object.prototype.toString.call([1]));
console.log("hasOwn", Object.prototype.hasOwnProperty.call({a:1}, "a"));
console.log("slice", Array.prototype.slice.call([1,2,3], 1).join(","));
console.log("funcToString", typeof Function.prototype.toString.call(function f(){}));
console.log("propIsEnum", Object.prototype.propertyIsEnumerable.call({a:1}, "a"));
console.log("valueOf", Object.prototype.valueOf.call({}) !== null);
