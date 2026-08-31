// A property is four things -- a value, and whether it may be written, enumerated and redefined --
// and only `Object.defineProperty` can say all four. An assignment always makes the same shape, so
// until a program defines a property by hand the attributes are invisible; the moment it does,
// every other operation has to agree about them. That is what this asks: the descriptor a
// definition installs, what the object reports back, and which redefinitions the language refuses.

// What a definition does not say about a *new* property is false, where an assignment would have
// made all three true. The lazy-getter idiom depends on the whole difference.
const defined: any = {};
Object.defineProperty(defined, "hidden", { value: 1 });
defined.assigned = 2;
console.log(Object.getOwnPropertyDescriptor(defined, "hidden"));
console.log(Object.getOwnPropertyDescriptor(defined, "assigned"));
console.log(Object.keys(defined), Object.getOwnPropertyNames(defined), JSON.stringify(defined));
console.log(Object.getOwnPropertyDescriptor(defined, "absent"), Object.hasOwn(defined, "hidden"));

// A non-writable property takes the write and drops it; a non-enumerable one is readable and
// invisible to everything that lists. Modules are strict and a real engine throws on the write,
// but both kiln engines are silent, and they have to agree before either can change.
defined.hidden = 99;
console.log(defined.hidden, "hidden" in defined, defined.assigned);

// An accessor descriptor is a different *kind* of property, not a data property with functions in
// it: it has get and set where the other has value and writable, and never both.
const accessor: any = {};
Object.defineProperty(accessor, "doubled", {
  get(): number { return 21; },
  enumerable: true,
  configurable: true,
});
const descriptor: any = Object.getOwnPropertyDescriptor(accessor, "doubled");
console.log(accessor.doubled, typeof descriptor.get, descriptor.set, "value" in descriptor, "writable" in descriptor);

// Redefining a *configurable* property is unrestricted -- including turning a data property into
// an accessor and back, which is the case that makes the two kinds one operation.
Object.defineProperty(accessor, "doubled", { value: 7, writable: true, configurable: true });
console.log(accessor.doubled, Object.getOwnPropertyDescriptor(accessor, "doubled"));

// Redefining a *non-configurable* one is refused by name. The one redefinition it still allows is
// the one that changes nothing, which is why each attribute is compared rather than the whole
// descriptor being rejected on sight.
const fixed: any = {};
Object.defineProperty(fixed, "x", { value: 1, writable: false, enumerable: false, configurable: false });
try { Object.defineProperty(fixed, "x", { value: 2 }); console.log("changed the value"); } catch (error: any) { console.log(error.name, error.message); }
try { Object.defineProperty(fixed, "x", { enumerable: true }); console.log("changed enumerable"); } catch (error: any) { console.log(error.name); }
try { Object.defineProperty(fixed, "x", { configurable: true }); console.log("un-fixed it"); } catch (error: any) { console.log(error.name); }
Object.defineProperty(fixed, "x", { value: 1, writable: false, enumerable: false, configurable: false });
console.log("a redefinition that changes nothing is allowed", fixed.x);

// Non-configurable but still writable is the weaker promise: the value may change, and it may be
// made non-writable once, which is the one attribute change a fixed property allows.
const settling: any = {};
Object.defineProperty(settling, "y", { value: 1, writable: true, enumerable: true, configurable: false });
settling.y = 5;
Object.defineProperty(settling, "y", { value: 6 });
console.log(settling.y, Object.getOwnPropertyDescriptor(settling, "y"));
Object.defineProperty(settling, "y", { writable: false });
console.log(Object.getOwnPropertyDescriptor(settling, "y"));
try { Object.defineProperty(settling, "y", { writable: true }); console.log("made it writable again"); } catch (error: any) { console.log(error.name); }

// Freezing is the same promise made about every property at once, so a definition afterwards is
// refused for the same reason -- and adding a name the object never had is refused too.
const frozen: any = Object.freeze({ a: 1 });
console.log(Object.isFrozen(frozen), Object.getOwnPropertyDescriptor(frozen, "a"));
try { Object.defineProperty(frozen, "a", { value: 2 }); console.log("redefined a frozen property"); } catch (error: any) { console.log(error.name); }
try { Object.defineProperty(frozen, "b", { value: 2 }); console.log("added to a frozen object"); } catch (error: any) { console.log(error.name); }
console.log(frozen.a, Object.keys(frozen));

// The plural forms, and the two-argument `Object.create`: the same operation applied to a map of
// descriptors, so a mistake about attributes shows up here as well as one property at a time.
const many: any = Object.defineProperties({}, {
  visible: { value: 1, enumerable: true },
  quiet: { value: 2 },
});
console.log(Object.keys(many), many.visible, many.quiet);
console.log(Object.getOwnPropertyDescriptors(many));
const created: any = Object.create(null, { only: { value: 3, enumerable: true } });
console.log(created.only, Object.keys(created), Object.getPrototypeOf(created));
