process.exitCode = "3";
console.log(process.exitCode, typeof process.exitCode);
setTimeout(() => {
  process.exitCode = 7;
  console.log(process.exitCode, typeof process.exitCode);
  process.exitCode = undefined;
  console.log(process.exitCode);
}, 0);
