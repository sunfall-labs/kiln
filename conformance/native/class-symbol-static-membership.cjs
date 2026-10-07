const brand = Symbol.for('kiln-static-membership');
const variance = {};
function make(ast) {
  return class Schema {
    static ast = ast;
    static [brand] = variance;
    static annotations() { return make(this.ast); }
    static get lazy() { throw new Error('membership must not invoke a getter'); }
  };
}
class Derived extends make('schema') {}
console.log(brand in Derived, Derived[brand] === variance, 'lazy' in Derived);
console.log(Reflect.ownKeys(Object.getPrototypeOf(Derived)).includes(brand));
const isSchema = value => brand in value && typeof value[brand] === 'object';
const dual = (predicate, body) => function () {
  if (predicate(arguments)) return body.apply(this, arguments);
  return self => body(self, ...arguments);
};
const transform = dual(args => isSchema(args[0]) && isSchema(args[1]), () => make('transformed'));
console.log(transform(Derived.annotations(), Derived).ast);
function isolated() {}
Object.setPrototypeOf(isolated, null);
console.log('call' in isolated, 'name' in isolated, 'prototype' in isolated, 'prototype' in (() => {}));
const bridge = Object.create(Derived);
console.log(brand in bridge, bridge[brand] === variance);
