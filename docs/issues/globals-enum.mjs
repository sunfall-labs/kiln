// Which node-flavoured globals enumerate their statics the way node does?
for (const name of ["Buffer", "process", "console"]) {
  const value = globalThis[name];
  if (!value) { console.log(name, "absent"); continue; }
  const forIn = []; for (const k in value) forIn.push(k);
  const own = Object.getOwnPropertyNames(value).filter((k) => !["length","name","prototype"].includes(k));
  console.log(name, "for-in:", forIn.length, "own:", own.length);
}
