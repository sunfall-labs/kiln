// kiln-native: expect-exit=1
// The same, from TypeScript: frames are in the authored file's coordinates, so the interface and
// the annotations above the throw do not shift them.
interface Shape {
  sides: number;
}

function check(shape: Shape): number {
  if (shape.sides < 3) {
    throw new TypeError("not a polygon: " + shape.sides);
  }
  return shape.sides;
}

console.log("before");
console.log(check({ sides: 2 }));
