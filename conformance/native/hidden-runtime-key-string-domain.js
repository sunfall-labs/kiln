// Runtime-private names are hidden only in the reserved internal-key domain. Ordinary strings
// with the same spelling remain observable through object literals, computed reads and own keys.
const names = [
  "@@node:socket",
  "@@node:server",
  "@@node:client",
  "@@node:response",
  "@@kiln:node:is-file",
  "@@kiln:node:is-directory",
];
const object = Object.fromEntries(names.map((name, index) => [name, index + 1]));
console.log("literal", object["@@node:socket"], object["@@kiln:node:is-directory"]);
console.log("computed", names.map((name) => object[name]).join(","));
console.log("keys", Object.keys(object).join(","));
console.log("near", object["@@node:socketx"], object["@@kiln:node:is-filex"]);
