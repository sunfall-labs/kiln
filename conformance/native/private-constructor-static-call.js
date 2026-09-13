// Private member declaration, branding, and access during and after construction,
// including calling static private methods from instance constructors.
class C {
  #n = 7;
  #m() {
    return this.#n * 2;
  }
  get #val() {
    return this.#n + 10;
  }

  static #c = 0;
  static #r() {
    C.#c += 1;
    return C.#c;
  }

  constructor(val) {
    if (val !== undefined) {
      this.#n = val;
    }
    C.#r();
  }

  n() {
    return this.#n;
  }
  m() {
    return this.#m();
  }
  val() {
    return this.#val;
  }
  static count() {
    return C.#c;
  }
}

const c1 = new C();
console.log(c1.n(), c1.m(), c1.val(), C.count());

const c2 = new C(42);
console.log(c2.n(), c2.m(), c2.val(), C.count());
