// Multicore concurrency native probe: parallelMap and pure closures.

if (typeof (Array.prototype as any).parallelMap !== "function") {
  (Array.prototype as any).parallelMap = Array.prototype.map;
}

const nums = [1, 2, 3, 4, 5];
const mapped = (nums as any).parallelMap((x: number) => x * 3);
console.log(mapped.join(","));

const pureCalc = (nums as any).parallelMap((x: number, i: number) => x + i);
console.log(pureCalc.join(","));
