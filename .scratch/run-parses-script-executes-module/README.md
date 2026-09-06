# `kiln run foo.js` parses as a Script but executes as a Module

**Status:** resolved.

**Resolution:**
`parse` (`crates/frontend-oxc/src/lib.rs`) now sets `.with_module(true)` for non-`.cjs` files, aligning parser early error checking with the module runtime semantics enforced by `kiln_ir::compile_module` and `kiln-module-graph`. `.cjs` continues to parse with CommonJS / Script grammar. Verified with unit tests, differential tests, regress tests, and native parity.

`parse` (`crates/frontend-oxc/src/lib.rs`) takes the source type from the path, and Oxc maps
`.js` to a *Script*. The runtime then reports the same file as a module -- its errors read
`could not execute module ...` -- and applies strict semantics: an assignment to an undeclared
name is a `ReferenceError` rather than a new global.

So the two halves disagree about what the file is:

```js
// foo.js
var o = {p: 1}; var p = 2; var r;
with (o) { r = p; }          // kiln runs it; node as a module: SyntaxError
console.log("with:", r);     // "with: 1"
```

```js
undeclaredXyz = 1;           // kiln: ReferenceError, which is the *module* rule
```

The early errors that only a Script permits -- `with`, octal literals, `delete` of a variable --
are therefore accepted in a file whose runtime is strict.

Test262 is unaffected: the conformance runner goes through `parse_with_goal`, which sets the goal
explicitly for each test, so both `.js`-as-script and `.js`-as-module are parsed correctly there.
The three `strict-mode` failures under `language/statements/with` are `unscopables`
binding-deletion tests and unrelated to this.

The fix is presumably to have `parse` choose the goal the runtime will use rather than the one
the extension implies, which is a decision about what `kiln run` means for a bare `.js` file --
worth making deliberately rather than inheriting from Oxc's extension convention.
