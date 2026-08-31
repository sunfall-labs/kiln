// `require` resolves only the specifiers a static pass already recorded for this module.
//
// Run as CommonJS (a `.js` file with no import/export). A direct `require("path")` works, but the
// alias every bundler emits -- `var __require = typeof require !== "undefined" ? require : ...`
// -- and any computed specifier throw `Cannot find module`. It is an ordinary catchable Error, so
// a library that wraps its builtin require in a try/catch degrades silently instead of failing.
//
// The second line is a separate wrong answer: the `require` binding is not identical to itself
// across two reads, which is the same shape as the `globalThis === globalThis` finding.
//
// node:  alias-is-require true / alias-only function / computed-only function
// kiln:  alias-is-require false / alias-only-threw Cannot find module 'path'
//                                / computed-only-threw Cannot find module 'os'
var __require = typeof require !== "undefined" ? require : null;
console.log("alias-is-require", __require === require);
try { console.log("alias-only", typeof __require("path").join); } catch (e) { console.log("alias-only-threw", e.message); }
try { console.log("computed-only", typeof require("o" + "s").platform); } catch (e) { console.log("computed-only-threw", e.message); }
