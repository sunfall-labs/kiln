function testSort() {
  const arr = [];
  for (let i = 0; i < 20000; i++) {
    arr.push((i * 1103515245 + 12345) % 65536);
  }
  arr.sort((a, b) => a - b);
  return arr[0] + arr[arr.length - 1];
}

console.log(testSort());
