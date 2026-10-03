const events = [];
const key = { [Symbol.toPrimitive](hint) { events.push(hint); return Symbol.for('key'); } };
const make = new Function('key', 'events',
  'return {[key]: (events.push("value"), 7), ["__proto__"]: 8, [Symbol.for("method")](x) { return x + 1; }}');
const result = make(key, events);
console.log(events.join(','), result[Symbol.for('key')], result.__proto__,
  Object.getPrototypeOf(result) === Object.prototype);
console.log(result[Symbol.for('method')](41), result[Symbol.for('method')].name);
try { new Function('return {["x": 1}'); } catch (error) { console.log(error.name); }
