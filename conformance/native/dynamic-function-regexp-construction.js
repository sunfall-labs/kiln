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

for (const source of [
  "if (false) /[/u;",
  "if (((false))) /[/u;",
  "while (false) /[/u;",
  "for (; false;) /[/u;",
  "return /unterminated",
  "return /line\nfeed/;",
  "return /a\\" + "\nb/;",
  "return /a\\" + String.fromCharCode(0x2028) + "b/;",
]) {
  let rejectedAtConstruction = false;
  try {
    Function(source);
  } catch (error) {
    rejectedAtConstruction = error instanceof SyntaxError;
  }
  if (!rejectedAtConstruction) {
    throw new Error(`invalid regex source was accepted: ${JSON.stringify(source)}`);
  }
}

const divide = Function("return (8) / 2;");
if (divide() !== 4) throw new Error("ordinary parenthesized expression remains division");
const callDivide = Function("return Number(8) / 2;");
if (callDivide() !== 4) throw new Error("ordinary call parentheses remain division");
