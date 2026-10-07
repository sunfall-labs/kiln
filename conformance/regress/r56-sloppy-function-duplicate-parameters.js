const duplicate = Function("a", "a", "return a;");
const fromStrictCaller = (function () {
  "use strict";
  return Function("a", "a", "return a;");
})();

if (duplicate(1, 2) !== 2 || fromStrictCaller(3, 4) !== 4) {
  throw new Error("sloppy Function constructor must bind the last duplicate parameter");
}

for (const body of ['"use strict"; return a;', "return a;"]) {
  let threw = false;
  try {
    if (body.includes("use strict")) {
      Function("a", "a", body);
    } else {
      Function("a", "a = 0", body);
    }
  } catch (error) {
    threw = error instanceof SyntaxError;
  }
  if (!threw) throw new Error("strict or non-simple duplicate parameters must be rejected");
}

console.log(duplicate.length, duplicate(1, 2), fromStrictCaller(3, 4));
