function f() { return () => arguments.length }
function g() { return [1,2].map(() => arguments.length) }
console.log(f(1,2)(), g('a','b','c').join(','));
