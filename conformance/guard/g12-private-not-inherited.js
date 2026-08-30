class C { static f() { return this.#g() } static #g() { return 42 } }
class D extends C {}
console.log(C.f());
try { D.f(); console.log('no throw') } catch (e) { console.log('threw', e.constructor.name) }
class A { #x = 7; #m() { return this.#x } run() { return this.#m() } check(o) { return #x in o } }
class B extends A {}
const branded = new B();
console.log('inherited instance private:', branded.run());
console.log('private brand:', branded.check(branded), branded.check({}));
