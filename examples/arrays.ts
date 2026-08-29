// Arrays are a distinct value, not an object with numeric keys: `Array.isArray` has to tell
// them apart and `length` is derived rather than stored.
const items = [1, 2, 3];
console.log("length", items.length, "index", items[0], items[2]);
console.log("past the end", items[9]);

items[1] = 20;
items[5] = 60;
console.log("assigned", items.join(","), "grew to", items.length);

// `a["0"]` and `a[0]` name the same element; `a[-1]` and `a[1.5]` do not.
console.log("string index", items["0"]);

// Reading a method twice yields the same function, which is why property reads are unbound and
// the receiver is supplied by the call.
console.log("method identity", [].values === [].values, [][Symbol.iterator] === [].values);
console.log("symbol key present", Symbol.iterator in items);

// Built-in statics.
console.log("isArray", Array.isArray(items), Array.isArray({}));
console.log("keys", Object.keys({ a: 1, b: 2 }).join(","));
console.log("typeof", typeof Array.from, typeof JSON.parse, typeof Symbol.iterator);
