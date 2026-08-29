class A extends Array {}
const a = new A(); a.push(1);
console.log(a instanceof Array, a.length);
class M extends Map {}
const m = new M(); m.set('k', 1);
console.log(m instanceof Map, m.get('k'));
