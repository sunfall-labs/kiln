const isArray = Function("value", "return value instanceof Array;");
if (!isArray([]) || isArray({})) throw new Error("dynamic instanceof uses the ordinary prototype chain");

const negated = Function("value", "return !(value instanceof Map);");
if (negated(new Map()) || !negated({})) throw new Error("instanceof has relational precedence");

let threw = false;
try { Function("return ({}) instanceof 1;")(); }
catch (error) { threw = error.name === "TypeError"; }
if (!threw) throw new Error("dynamic instanceof validates the right-hand operand");

console.log("dynamic instanceof: array, negation, invalid right-hand side");
