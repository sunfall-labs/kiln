const map = new Map();
const set = new Set();

for (let i = 0; i < 50000; i++) {
  map.set(i, i * 2);
  set.add(i);
}

let sum = 0;
for (let i = 0; i < 50000; i++) {
  if (set.has(i)) {
    sum += map.get(i);
  }
}

console.log(sum);
