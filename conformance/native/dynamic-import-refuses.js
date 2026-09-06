// Dynamic import `import(specifier)` is refused in the native backend because a generated unit
// is a closed object file where the module graph is fixed at emit time.
console.log("before");
import("./nonexistent.mjs").catch(e => console.log("caught", e.message));
console.log("unreachable in native");
