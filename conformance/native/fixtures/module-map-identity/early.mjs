// Evaluated before the entry reaches late.mjs statically: the dynamic import is late.mjs's one
// evaluation, and the static import links to it.
export const early = await import("./late.mjs");
