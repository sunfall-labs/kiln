// A method borrowed from `Array.prototype` is generic, and its result is built the way
// `ArraySpeciesCreate` builds one: an ordinary Array for any receiver that is not an Array, a
// typed view included. `%TypedArray%.prototype.map` is a different function that builds its
// result with `TypedArraySpeciesCreate` and so answers a view of the receiver's kind. The
// interpreter told the two apart only when the method was reached by a property read, so
// `Array.prototype.map.call(view, f)` answered a view (wrong) and
// `%TypedArray%.prototype.map.call(view, f)` answered an array (wrong in the other direction)
// once `call`, `apply`, `bind` or `Reflect.apply` carried it there.
const view = () => new Float64Array([1, 2, 3, 4]);
const TA = Object.getPrototypeOf(Float64Array.prototype);
const shape = (r) =>
  Array.isArray(r) ? "Array" : r && r.constructor ? r.constructor.name : String(r);
const show = (name, r) => console.log(name, shape(r), JSON.stringify(Array.from(r)));

// Borrowed from `Array.prototype`: an ordinary Array, however it is carried to the receiver.
show("map.call", Array.prototype.map.call(view(), (x) => x * 2));
show("filter.call", Array.prototype.filter.call(view(), (x) => x > 2));
show("map.apply", Array.prototype.map.apply(view(), [(x) => x * 2]));
show("map.bind", Array.prototype.map.bind(view())((x) => x * 2));
show("Reflect.apply(map)", Reflect.apply(Array.prototype.map, view(), [(x) => x * 2]));
show("slice.call", Array.prototype.slice.call(view(), 1, 3));
show("toSorted.call", Array.prototype.toSorted.call(view(), (a, b) => b - a));
show("toReversed.call", Array.prototype.toReversed.call(view()));
show("toSpliced.call", Array.prototype.toSpliced.call(view(), 1, 1));
show("with.call", Array.prototype.with.call(view(), 0, 99));
show("flat.call", Array.prototype.flat.call(view()));
show("flatMap.call", Array.prototype.flatMap.call(view(), (x) => [x]));

// The view's own methods: a view of the same kind, however they are carried.
show("view.map", view().map((x) => x * 2));
show("view.filter", view().filter((x) => x > 2));
show("view.slice", view().slice(1, 3));
show("TA.map.call", TA.map.call(view(), (x) => x * 2));
show("TA.filter.call", TA.filter.call(view(), (x) => x > 2));
show("TA.slice.call", TA.slice.call(view(), 1, 3));
show("TA.map.bind", TA.map.bind(view())((x) => x * 2));
show("Reflect.apply(TA.map)", Reflect.apply(TA.map, view(), [(x) => x * 2]));
show("TA.toSorted.call", TA.toSorted.call(view(), (a, b) => b - a));
show("TA.toReversed.call", TA.toReversed.call(view()));
show("TA.with.call", TA.with.call(view(), 0, 99));

// A subclass keeps its own species through the view's methods, and loses it through the
// generic ones, which is the same distinction one level down.
class MyF64 extends Float64Array {}
show("subclass.map", new MyF64([1, 2]).map((x) => x * 2));
show("Array.map.call(subclass)", Array.prototype.map.call(new MyF64([1, 2]), (x) => x * 2));

// An ordinary array receiver is untouched by any of it, and the in-place rearrangement a view
// borrows from `Array.prototype` still writes through to the view rather than to a snapshot.
show("array.map", [1, 2, 3].map((x) => x * 2));
const reversed = view();
const back = Array.prototype.reverse.call(reversed);
console.log("reverse.call", shape(back), back === reversed, JSON.stringify(Array.from(reversed)));
