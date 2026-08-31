// @babel/parser + @babel/traverse + @babel/generator: parse JavaScript, rewrite the AST, print it.
//
// acorn already parses; this goes further. Babel's parser is a large class hierarchy assembled by
// successive `class X extends Y` mixins, traverse builds visitor tables with getters and
// `Object.defineProperty`, and the generator is a printer driven by a lookup table of node types.
// The whole toolchain is regular-expression and Unicode-property heavy.
import { parse } from "@babel/parser";
import _traverse from "@babel/traverse";
import _generate from "@babel/generator";

const traverse = _traverse.default ?? _traverse;
const generate = _generate.default ?? _generate;

const src = `
class Counter extends Base {
  static #instances = 0;
  #n = 0;
  constructor(n = 0) { super(); this.#n = n; Counter.#instances++; }
  get value() { return this.#n; }
  *[Symbol.iterator]() { yield* [this.#n, this.#n + 1]; }
  async next() { for await (const x of src()) { this.#n += x ?? 0; } return this?.#n; }
}
const { a, b: [c = 1, ...rest], ...others } = obj;
const re = /(?<year>\\d{4})-(?<m>\\d{2})/gu;
label: for (const k of keys) { if (k) continue label; else break label; }
export default async function* gen(...args) { yield await Promise.all(args.map((x) => x ** 2)); }
const t = \`a\${1 + 2}b\${x?.y?.()}\`;
try { throw new Error("x"); } catch { } finally { }
`;

const ast = parse(src, { sourceType: "module", plugins: ["classProperties", "classPrivateProperties", "classPrivateMethods"] });

let count = 0;
const byType = new Map();
traverse(ast, {
  enter(path) {
    count++;
    byType.set(path.node.type, (byType.get(path.node.type) ?? 0) + 1);
  },
});
console.log("nodes", count);
console.log("types", [...byType.entries()].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).slice(0, 6).map(([k, v]) => k + ":" + v).join(","));

// Rewrite: rename every `Identifier` named `obj` and double every numeric literal.
traverse(ast, {
  Identifier(path) { if (path.node.name === "obj") path.node.name = "renamed"; },
  NumericLiteral(path) { path.node.value = path.node.value * 2; },
});

const out = generate(ast, { compact: false, concise: false }).code;
console.log("gen-lines", out.split("\n").length);
console.log("gen-has-renamed", out.includes("renamed"), "gen-has-private", out.includes("#instances"));
console.log("gen-regex", /\(\?<year>/.test(out));
console.log("gen-optional-chain", out.includes("?.") , "gen-nullish", out.includes("??"));

// Error recovery: a deliberate syntax error must produce a located SyntaxError, not a crash.
try {
  parse("const = ;", { sourceType: "module" });
  console.log("parse-error none");
} catch (e) {
  console.log("parse-error", e.constructor.name, /\(1:6\)/.test(e.message) || /1:6/.test(String(e.message)));
}

// Unicode identifiers and property escapes, which is where a parser leans hardest on regex.
const uni = parse("const \\u0061\\u{62} = 'é中'; const λ = 1;", { sourceType: "module" });
console.log("unicode-decls", uni.program.body.length, uni.program.body[0].declarations[0].id.name);
