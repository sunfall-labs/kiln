// kiln eval smoke test (authored for kiln, not from porffor).
import {type} from 'arktype';
const Name = type('string');
console.log(JSON.stringify(Name('Ada')));
console.log(Name(42).toString().slice(0, 30));
const Age = type('number');
console.log(JSON.stringify(Age(30)));
console.log(Age('x').toString().slice(0, 30));
const Person = type({name: 'string', age: 'number'});
console.log(JSON.stringify(Person({name: 'Bo', age: 7})));
console.log(Person({name: 'Bo'}).toString().slice(0, 40));
