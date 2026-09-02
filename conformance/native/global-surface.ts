// Every name the realm publishes on its global object, and what `typeof` says about each.
//
// WHY THIS PROBE IS A TABLE AND NOT A LIST OF NAMES. `typeof X === "undefined"` is how real code
// feature-detects. A compiled program that answers `undefined` where the interpreter answers
// `function` does not fail -- it silently takes the *other* branch, which is a divergence that
// nothing in this corpus was asking about. Three real divergences reached main this way. So the
// probe does not name the globals it checks: it asks the realm for its own list with
// `Object.getOwnPropertyNames(globalThis)` and prints one row per name. A global added to one
// engine and not the other changes the number of rows, and the two engines stop printing the
// same bytes. Nobody has to remember to add it here.
//
// `$262` is skipped on purpose: those are the Test262 host hooks the runner installs, a statement
// about what the *host* can do rather than about the language, and they are not the native
// backend's to carry.
const names = Object.getOwnPropertyNames(globalThis)
  .filter((name: string) => !name.startsWith("$262"))
  .sort();
console.log(names.length);
for (const name of names) {
  console.log(name, typeof (globalThis as Record<string, unknown>)[name]);
}
