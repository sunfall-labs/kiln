const proto = { [Symbol.iterator]() { return { next: () => ({ done: true }) } }, foo() { return 'f' }, tag: 'T' };
function K() {}
Object.setPrototypeOf(K, proto);
class Sub extends K {}
console.log(typeof K[Symbol.iterator], K.foo(), K.tag, typeof Sub[Symbol.iterator], Sub.tag);
