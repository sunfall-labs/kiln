// What a timer handle looks like to a program. The engine allocates it as the timer's own record,
// not as an ordinary object, so everything a program can ask of an object has to be answered for
// it: its type, prototype chain, own properties, conversion and identity. The two engines are
// compared with each other here; node's own `Timeout` has many own fields that kiln's does not.
const t = setTimeout(() => {}, 1000);
t.unref();
const other = setTimeout(() => {}, 1000).unref();
const imm = setImmediate(() => {});
console.log(typeof t, t instanceof Object, t.constructor.name, imm.constructor.name);
console.log(Object.getPrototypeOf(t) === Object.getPrototypeOf(other), Object.getPrototypeOf(t) === Object.getPrototypeOf(imm));
console.log("unref" in t, "nope" in t, Object.keys(t).length, JSON.stringify(t));
t.custom = 5;
console.log(t.custom, Object.keys(t), Object.hasOwn(t, "custom"), delete t.custom, t.custom);
console.log(+t === +t, String(t) === String(+t), `${t}` === String(+t), t == +t, t === t, t === other);
console.log(Object.prototype.toString.call(t), typeof t.hasRef(), typeof t.refresh, typeof t.close);
console.log(new Map([[t, 1]]).get(t), new Set([t, t, other]).size, Object.isFrozen(t));
console.log({ ...t }, Object.assign({}, t), Object.getOwnPropertyNames(t));
console.log(t.hasRef(), t.ref().hasRef(), t.unref().hasRef(), t.close() === t, t.hasRef());
console.log(imm.hasRef(), imm.unref().hasRef(), imm.ref() === imm);
clearTimeout(other);
clearTimeout(+t);
clearImmediate(imm);
console.log(!!t, t ? "truthy" : "falsy", t ?? "nullish");
