// Tier A (pure compute, no imports): runs on node, bun, kiln, porffor, hermes.
//
// Synthetic user-table transform: deterministic LCG dataset, per-city
// aggregates, top-5 oldest. Stdout is the checksum; every runtime must print
// it byte-for-byte.
type User = { id: number; name: string; city: string; age: number };

let seed = 1234567;
function next(n: number): number {
  seed = (seed * 1103515245 + 12345) % 2147483648;
  return seed % n;
}

const CITIES: string[] = ["lima", "oslo", "quito", "riga", "suva", "tampa"];
const users: User[] = [];
for (let i = 0; i < 5000; i++) {
  users.push({
    id: i,
    name: "user" + i,
    city: CITIES[next(CITIES.length)],
    age: 18 + next(60),
  });
}

const countByCity: Record<string, number> = {};
const ageSumByCity: Record<string, number> = {};
for (const u of users) {
  countByCity[u.city] = (countByCity[u.city] ?? 0) + 1;
  ageSumByCity[u.city] = (ageSumByCity[u.city] ?? 0) + u.age;
}

const sorted = [...CITIES].sort();
for (const c of sorted) {
  const avg = ageSumByCity[c] / countByCity[c];
  console.log(`${c} count=${countByCity[c]} avgAge=${avg.toFixed(2)}`);
}

const byAge = [...users].sort((a, b) => b.age - a.age || a.id - b.id).slice(0, 5);
for (const u of byAge) console.log(`top id=${u.id} age=${u.age} city=${u.city}`);
console.log(`total=${users.length}`);
