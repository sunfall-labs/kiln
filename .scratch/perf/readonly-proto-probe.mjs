"use strict";
const proto = {};
Object.defineProperty(proto, "locked", { value: 1, writable: false });
const child = Object.create(proto);
let refused = false;
try { child.locked = 2; } catch (e) { refused = e instanceof TypeError; }
console.log("refused", refused, "own", Object.hasOwn(child, "locked"), "value", child.locked);
const plain = Object.create({ open: 1 });
plain.open = 2;
console.log("shadowed", Object.hasOwn(plain, "open"), plain.open);
