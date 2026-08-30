import origin, { Box, describe, double } from "./shapes.ts";

// The imported `unit` is not in scope here, and must not be: each function reads its own
// module's binding, not the importer's.
const unit = "in";

console.log(describe(3));
console.log(new Box(4).label());
console.log(double(21));
console.log(origin(), unit);
