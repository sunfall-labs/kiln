// Spread calls must enter compiled VM frames without consuming native callback depth.
function descend(n) { return n ? 1 + descend(...[n - 1]) : 0; }
const bound = descend.bind(null);
console.log(descend(256), bound(...[256]));
const receiver = { marker: 7, method(a, b) { return this.marker + a + b; } };
console.log(receiver.method(...[2, 3]));
class Parent { method(a, b) { return this.marker + a + b; } }
class Child extends Parent { constructor() { super(); this.marker = 11; }
    method(...args) { return super.method(...args); } }
console.log(new Child().method(...[2, 3]));
const failure = {};
function throws() { throw failure; }
try { throws(...[]); } catch (error) { console.log(error === failure); }
function strict() { 'use strict'; return this === undefined; }
console.log(strict(...[]));
console.log(Math.max(...[3, 9, 1]));
    
