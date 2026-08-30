import { Crate, describe, double, origin, shapes, millimetre, system } from "./reexports.ts";
import * as everything from "./shapes.ts";

console.log(describe(1), double(2), origin(), new Crate(3).label());
console.log(millimetre, system);
console.log(Object.keys(shapes).join(","));
console.log(everything.default === origin);
