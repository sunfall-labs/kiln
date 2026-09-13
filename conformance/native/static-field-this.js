// Static field initialisers evaluate with the constructor as `this`, including
// arrows created and invoked by the initialiser. Layer's Context.Service uses
// this contract when its static memo-map service is created.
class Service {
  static self = this;
  static arrow = (() => this)();
  static delayed = () => this;
}

console.log(Service.self === Service, Service.arrow === Service);

const delayed = Service.delayed;
console.log(delayed() === Service);

class Derived extends Service {
  static self = this;
}
console.log(Derived.self === Derived, Derived.self === Service);

const receiver = { key: "computed" };
const Computed = (function () {
  return class {
    static [this.key] = this;
  };
}).call(receiver);
console.log(Computed.computed === Computed, Computed.computed === receiver);
