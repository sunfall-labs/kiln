// An empty collection still validates forEach's callback, but it must not invoke it.
function attempt(label, body) {
  try { body(); console.log(label, "ok"); }
  catch (error) { console.log(label, error instanceof TypeError ? "TypeError" : error.name); }
}

const emptyMap = new Map();
const emptySet = new Set();
attempt("empty-map-noncallable", () => emptyMap.forEach(1));
attempt("empty-set-noncallable", () => emptySet.forEach(null));

const deletedMap = new Map([["a", 1], ["b", 2]]);
deletedMap.delete("a");
deletedMap.delete("b");
const deletedSet = new Set([1, 2]);
deletedSet.delete(1);
deletedSet.delete(2);
attempt("deleted-map-noncallable", () => deletedMap.forEach({}));
attempt("deleted-set-noncallable", () => deletedSet.forEach(undefined));

let calls = 0;
emptyMap.forEach(() => { calls++; });
emptySet.forEach(() => { calls++; });
deletedMap.forEach(() => { calls++; });
deletedSet.forEach(() => { calls++; });
class ConstructOnly {}
emptyMap.forEach(ConstructOnly);
const revoked = Proxy.revocable(() => {}, {});
revoked.revoke();
emptySet.forEach(revoked.proxy);
console.log("empty-calls", calls);

// Nonempty iteration must remain live: newly added entries are visited.
const changing = new Map([["first", 1]]);
const visited = [];
changing.forEach((value, key) => {
  visited.push(`${key}:${value}`);
  if (key === "first") changing.set("second", 2);
});
console.log("mutated", visited.join(","));
