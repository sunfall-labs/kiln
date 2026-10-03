"use strict";
class ClockDate extends Date { static clock; }
const clock = { now: 123 };
const date = new Proxy(ClockDate, { apply() { return "fake"; } });
date.clock = clock;
date.extra = 7;
console.log(date.clock === clock, ClockDate.clock === clock, ClockDate.extra, date());

const events = [];
const target = function target() {};
const proxy = new Proxy(target, {
  getOwnPropertyDescriptor(object, key) {
    events.push("describe:" + key);
    return Reflect.getOwnPropertyDescriptor(object, key);
  },
  defineProperty(object, key, descriptor) {
    events.push("define:" + key + ":" + Object.keys(descriptor).join(","));
    return Reflect.defineProperty(object, key, descriptor);
  },
});
proxy.value = 3;
proxy.value = 4;
console.log(target.value, events.join("|"));
let setterReceiver;
Object.defineProperty(target, "sink", {
  set(value) { setterReceiver = this; this.value = value; },
});
proxy.sink = 8;
console.log(setterReceiver === proxy, target.value);
Object.defineProperty(target, "locked", { value: 1 });
try { proxy.locked = 2; } catch (error) { console.log(error instanceof TypeError); }
const refuses = new Proxy(function () {}, { defineProperty() { return false; } });
try { refuses.fresh = 1; } catch (error) { console.log(error instanceof TypeError); }
Object.preventExtensions(target);
try { proxy.fresh = 1; } catch (error) { console.log(error instanceof TypeError); }
console.log(Object.hasOwn(target, "fresh"));
