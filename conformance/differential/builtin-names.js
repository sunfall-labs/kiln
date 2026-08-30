// A console names a runtime-supplied function by its JavaScript name, not by whatever the
// implementation happens to call it. kiln derives that name from the variant rather than keeping
// a table of two hundred of them, so this is the check that the derivation is right -- and it is
// deliberately broad, because a derivation that is wrong for one family is wrong silently.
const array = [1];
const text = "x";
const number = 1;
const object = {};
const map = new Map();
const set = new Set();

console.log(array.push, array.pop, array.join, array.indexOf, array.includes, array.slice);
console.log(array.map, array.filter, array.reduce, array.forEach, array.sort, array.find);
console.log(array.findIndex, array.some, array.every, array.concat, array.flat, array.entries);
console.log(text.toUpperCase, text.slice, text.split, text.trim, text.startsWith);
console.log(text.replace, text.charCodeAt, text.padStart, text.repeat, text.indexOf);
console.log(number.toFixed, number.toString, number.valueOf);
console.log(object.hasOwnProperty, object.toString, object.valueOf, object.isPrototypeOf);
console.log(Object.keys, Object.assign, Object.freeze, Object.entries, Object.getPrototypeOf);
console.log(JSON.parse, JSON.stringify, Math.floor, Math.max, Math.abs);
console.log(Number.isInteger, Number.parseFloat, Array.isArray, Array.from);
console.log(map.set, map.get, map.has, map.delete, set.add, set.has);
console.log(Promise.resolve, Promise.all, Promise.race);
console.log(String, Number, Boolean, Object, Array, Symbol, Promise);
// The globals that belong to no namespace, including the four whose spelling carries an acronym
// the variant name flattens -- `DecodeUri` reads back as `decodeUri` unless something says
// otherwise, and nothing else in this file would have caught that.
console.log(parseInt, parseFloat, isNaN, isFinite, escape, unescape);
console.log(encodeURI, encodeURIComponent, decodeURI, decodeURIComponent);
console.log(TypeError, RangeError, ReferenceError, SyntaxError, Map, Set, Date);
// A namespace is an object holding functions, so it shows as one rather than as a callable.
console.log(Math, JSON, typeof Math, String(Math), `${JSON}`);

// A constructor is named for its type; a method reached through an instance is named for itself.
console.log(`${[].push}`.length > 0, typeof [].push, [].push === [].push);
