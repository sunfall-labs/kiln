function dist(x: number, y: number): number {
  return Math.sqrt(x * x + y * y);
}

function untypedDist(x: any, y: any): any {
  return Math.sqrt(x * x + y * y);
}

console.log(dist(3, 4));
console.log(untypedDist(3, 4));
console.log(dist(5, 12) === untypedDist(5, 12));
console.log(dist(0, 0));
console.log(dist(-3, 4));

// Guard speculation with non-numbers deopting cleanly to generic clone
console.log(dist("3" as any, 4));
console.log(untypedDist("3", 4));
console.log(dist("3" as any, 4) === untypedDist("3", 4));
