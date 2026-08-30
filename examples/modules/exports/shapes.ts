// Everything a module can export that is not a scalar: a function, a class, an arrow, and a
// default. Each closes over module scope, which is the part that only works if a closure
// carries its own module rather than borrowing the caller's.
const unit = "cm";

export function describe(width: number): string {
  return width + unit;
}

export class Box {
  width: number;
  constructor(width: number) {
    this.width = width;
  }
  label(): string {
    return "box " + describe(this.width);
  }
}

export const double = (width: number): number => width * 2;

export default function origin(): string {
  return "0" + unit;
}
