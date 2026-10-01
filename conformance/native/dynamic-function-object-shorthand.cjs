const capture = Function("scope", "with (scope) { return function() { return { value }; }; }");
const scope = { value: 42 };
const read = capture(scope);
if (read().value !== 42) throw new Error("object shorthand missed captured with binding");
scope.value = 43;
if (read().value !== 43) throw new Error("object shorthand lost live binding");
let missing = false;
try { Function("return { __kiln_missing_shorthand_name__ }; ")(); }
catch (error) { missing = error instanceof ReferenceError; }
if (!missing) throw new Error("unresolved shorthand must throw ReferenceError");
const bound = Function("value", "return { value };")(undefined);
if (!("value" in bound) || bound.value !== undefined) throw new Error("bound undefined shorthand missing");
console.log("dynamic-object-shorthand", read().value, missing, "value" in bound);
