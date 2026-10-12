// Stack positions in a TypeScript file are the authored ones.
type Point = { x: number; y: number };

function make(point: Point): Error {
  const label: string = `(${point.x}, ${point.y})`;
  return new Error("at " + label);
}

function caller(): Error {
  return make({ x: 1, y: 2 });
}

console.log(caller().stack);
