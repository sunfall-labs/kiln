// A later realm may construct process.env from a process descriptor, but its values, new
// properties and deletions belong to that realm. This also exercises dictionary fallback.
const env = process.env;
const names = Object.keys(env);
const existing = names.find((name) => name !== "KILN_ENV_LAYOUT_PROBE_Q");
console.log(typeof env, existing !== undefined);
if (existing !== undefined) {
  const original = env[existing];
  const descriptor = Object.getOwnPropertyDescriptor(env, existing);
  console.log(descriptor?.value === original, descriptor?.enumerable === true);
  env[existing] = "kiln-probe";
  console.log(env[existing]);
  env[existing] = original;
  console.log(env[existing] === original);
}
const key = "KILN_ENV_LAYOUT_PROBE_Q";
const had = Object.prototype.hasOwnProperty.call(env, key);
const prior = env[key];
env[key] = "new";
console.log(env[key], Object.keys(env).at(-1) === key);
delete env[key];
console.log(Object.prototype.hasOwnProperty.call(env, key));
if (had) env[key] = prior;
