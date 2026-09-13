// Static field initialisers evaluate with the constructor as `this`, including
// arrows created and invoked by the initialiser. Layer's Context.Service uses
// this contract when its static memo-map service is created.
class Service {
  static self = this;
  static arrow = (() => this)();
}

console.log(Service.self === Service, Service.arrow === Service);
