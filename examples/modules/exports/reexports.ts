// Everything a module can forward: a named re-export, a rename, the default under a new name,
// a namespace, and a star. None of these names is bound in this module.
export { describe, double } from "./shapes.ts";
export { Box as Crate } from "./shapes.ts";
export { default as origin } from "./shapes.ts";
export * as shapes from "./shapes.ts";
export * from "./units.ts";

// A name this module exports itself wins over the same name arriving through the star.
export const system = "metric";
