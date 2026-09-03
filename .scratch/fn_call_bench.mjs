function add(a, b) {
  return a + b;
}
let sum = 0;
for (let i = 0; i < 15000000; i++) {
  sum = add(sum, i);
}
console.log(sum);
