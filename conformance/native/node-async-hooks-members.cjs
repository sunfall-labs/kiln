// What `require("node:async_hooks")` publishes, and what functions answer.
const hooks = require("node:async_hooks");

const names = Object.getOwnPropertyNames(hooks).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof hooks[name]);
}

console.log("default self-link:", hooks.default === hooks);

const als = new hooks.AsyncLocalStorage();
console.log("als.run:", typeof als.run);
console.log("als.getStore:", typeof als.getStore);
console.log("als.enterWith:", typeof als.enterWith);
console.log("als.exit:", typeof als.exit);
console.log("als.disable:", typeof als.disable);

console.log("als outside:", als.getStore());
const syncResult = als.run({ id: 42 }, () => als.getStore().id);
console.log("als sync:", syncResult);
console.log("als restored:", als.getStore());

const res = new hooks.AsyncResource("test");
console.log("res.bind:", typeof res.bind);
console.log("res.emitDestroy:", typeof res.emitDestroy);
const boundFn = res.bind((x) => x * 3);
console.log("bound call:", boundFn(7));
