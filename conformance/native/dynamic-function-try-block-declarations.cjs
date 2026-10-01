// A try body is a lexical block: declarations are instantiated before its statements run.
const render = Function(
  "",
  "try { function append(value) { return '<' + value + '>'; } return append('ok'); } catch (error) { return error.name; }"
);
const rendered = render();
if (rendered !== "<ok>") throw new Error("try-block function declaration was not initialized");
console.log("try-block-function", rendered);

const lexical = Function(
  "",
  "try { return value; const value = 3; } catch (error) { return error.name; }"
);
if (lexical() !== "ReferenceError") throw new Error("try-block lexical declaration missed TDZ");
console.log("try-block-tdz", lexical());

const catchFunction = Function(
  "",
  "try { throw 1; } catch (error) { function recover() { return error + 1; } return recover(); }"
);
if (catchFunction() !== 2) throw new Error("catch-block function declaration was not initialized");
console.log("catch-block-function", catchFunction());

const finallyFunction = Function(
  "",
  "try { return 0; } finally { function replace() { return 4; } return replace(); }"
);
if (finallyFunction() !== 4) throw new Error("finally-block function declaration was not initialized");
console.log("finally-block-function", finallyFunction());
