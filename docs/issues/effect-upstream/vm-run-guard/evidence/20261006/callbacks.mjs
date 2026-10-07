const count = 1_000_000;
const values = new Array(count).fill(1);
let checksum = 0;
values.map((value) => {
  checksum += value;
  return value;
});
console.log(checksum);
