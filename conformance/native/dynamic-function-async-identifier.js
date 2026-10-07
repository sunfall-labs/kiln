const generated = Function("async", "return async.invoke();");
const value = generated({ invoke() { return "contextual"; } });
console.log(value);
