// Dynamic import `import(specifier)` is refused in the native backend because a generated unit
// is a closed object file where the module graph is fixed at emit time.
console.log("before");
// The interpreter has a loader but no referrer directory for this standalone probe; a native
// object file has no run-time loader at all. Both are truthful, catchable rejections, so assert
// that shared contract without pretending their host diagnostics should be identical.
import("./nonexistent.mjs").catch(() => console.log("caught"));
console.log("unreachable in native");
