import { writeFileSync } from "node:fs";

// Fixture for file-cli.ts: 500 records, two deliberately bad.
const cities = ["lima", "oslo", "quito", "riga"];
let seed = 987654321;
const next = (n) => {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return seed % n;
};
const users = [];
for (let i = 0; i < 498; i++) {
  users.push({ name: "user" + i, age: 18 + next(60), city: cities[next(4)] });
}
users.push({ name: "bad-no-age", city: "lima" });
users.push({ name: "bad-no-city", age: 30 });
writeFileSync(new URL("./users.json", import.meta.url), JSON.stringify(users) + "\n");
console.log("wrote users.json with " + users.length + " records");
