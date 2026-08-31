// Property mutation carries the body strictness into the native operation.
const frozen = Object.freeze({ own: 1 });
try { frozen.own = 2; } catch (error) { console.log("frozen", error.name, frozen.own); }

const prototype = {};
Object.defineProperty(prototype, "fixed", { value: 1, writable: false });
Object.defineProperty(prototype, "read", { get() { return 1; } });
const child = Object.create(prototype);
try { child.fixed = 2; } catch (error) { console.log("inherited", error.name); }
try { child.read = 2; } catch (error) { console.log("getter", error.name); }

const array = Object.freeze([1]);
try { array[0] = 2; } catch (error) { console.log("array", error.name, array[0]); }
try { array.named = 2; } catch (error) { console.log("array-named", error.name); }

const fn = Object.freeze(function () {});
try { fn.extra = 1; } catch (error) { console.log("function", error.name); }
try { fn.prototype = {}; } catch (error) { console.log("function-prototype", error.name); }

const proxy = new Proxy({}, { set() { return false; } });
try { proxy.value = 1; } catch (error) { console.log("proxy", error.name); }
console.log(Reflect.set(frozen, "own", 2), Reflect.set(proxy, "value", 1));

const fixed = {};
Object.defineProperty(fixed, "x", { value: 1, configurable: false });
try { delete fixed.x; } catch (error) { console.log("delete", error.name); }
