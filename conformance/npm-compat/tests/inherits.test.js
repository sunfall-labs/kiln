// kiln eval smoke test (authored for kiln, not from porffor).
const inherits = require('inherits');
function Animal(name) { this.name = name; }
Animal.prototype.speak = function () { return this.name + ' speaks'; };
function Dog(name) { Dog.super_.call(this, name); }
inherits(Dog, Animal);
Dog.prototype.speak = function () { return this.name + ' barks'; };
const d = new Dog('Rex');
console.log(d instanceof Animal, d instanceof Dog, d.speak());
console.log(Dog.super_ === Animal);
console.log(new Animal('Cat').speak());
