const lone = String.fromCharCode(0xd800);
const slash = "\\";

for (const flags of ["u", "v"]) {
  let rejectedAtConstruction = false;
  try {
    // Function validates regex syntax even when execution could never reach the literal.
    Function(`if (false) return /${slash}${lone}/${flags};`);
  } catch (error) {
    rejectedAtConstruction = error instanceof SyntaxError;
  }
  if (!rejectedAtConstruction) {
    throw new Error(`backslash followed by a raw lone surrogate must be invalid under ${flags}`);
  }

  const validRaw = Function(`return /${lone}/${flags};`)();
  if (!validRaw.test(lone)) {
    throw new Error(`an unescaped raw lone surrogate remains valid under ${flags}`);
  }
}

// In a string literal, the same backslash/lone-surrogate pair is a valid identity escape.
const stringValue = Function(`return "${slash}${lone}";`)();
if (stringValue !== lone) throw new Error("string literal transport preserves its identity escape");

console.log("dynamic regexp construction:", stringValue.length, /./.test("x"));
