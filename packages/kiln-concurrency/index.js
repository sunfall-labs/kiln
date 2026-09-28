// Compatibility import. Kiln owns the actor runtime; this package has no local proxy,
// identity transfer helper or Array.prototype mutation.
export { Actor, spawnActor, connectActor, isolate, transfer } from "kiln:actors";
