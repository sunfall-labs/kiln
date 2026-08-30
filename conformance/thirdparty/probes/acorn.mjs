import { parse } from "acorn";
const tree = parse("const a = 1; function f(x) { return x * 2; } class C { m() {} }", { ecmaVersion: 2022 });
console.log(tree.type, tree.body.length);
console.log(tree.body.map((n) => n.type).join(","));
const fn = tree.body[1];
console.log(fn.id.name, fn.params.length, fn.body.body[0].type);
console.log(JSON.stringify(parse("1 + 2 * 3", { ecmaVersion: 2022 }).body[0].expression.operator));
