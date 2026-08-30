class C { static f() { return this.#g() } static #g() { return 42 } }
class D extends C {}
console.log(C.f());
try { D.f(); console.log('no throw') } catch (e) { console.log('threw', e.constructor.name) }
class A { #m() { return 1 } run() { return this.#m() } }
class B extends A {}
console.log('inherited instance private:', new B().run());
