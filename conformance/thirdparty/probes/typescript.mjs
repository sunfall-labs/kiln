// The TypeScript compiler API compiling a small source string.
//
// ~11 MB of machine-generated JavaScript in one file: a hand-written scanner, a recursive-descent
// parser, a checker built out of closures over huge tables, and an emitter. It is the largest and
// most language-complete input in the corpus, and it is the shape a real TypeScript build runs.
import ts from "typescript";

const source = [
  "export interface Point { x: number; y: number }",
  "export const origin: Point = { x: 0, y: 0 };",
  "export function shift<T extends Point>(p: T, dx: number): T {",
  "  return { ...p, x: p.x + dx };",
  "}",
  "export class Box<T> {",
  "  #items: T[] = [];",
  "  add(item: T): this { this.#items.push(item); return this; }",
  "  get size(): number { return this.#items.length; }",
  "}",
  "export async function* stream(n: number) { for (let i = 0; i < n; i++) yield i; }",
  "enum Dir { Up = 1, Down }",
  "export const d = Dir.Down;",
].join("\n");

// 1. Transpile: scanner + parser + transformers + emitter, no checker.
const out = ts.transpileModule(source, {
  compilerOptions: {
    target: ts.ScriptTarget.ES2017,
    module: ts.ModuleKind.ESNext,
    newLine: ts.NewLineKind.LineFeed,
  },
});
const emitted = out.outputText.replace(/\r\n/g, "\n").trim();
console.log("emit-lines", emitted.split("\n").length);
console.log("emit-has-await-of", emitted.includes("__asyncGenerator") || emitted.includes("async function*"));
console.log("emit-enum", /Dir\[Dir\["Up"\] = 1\] = "Up"/.test(emitted));

// 2. Parse to an AST and walk it: the parser's node factory plus visitor recursion.
const sf = ts.createSourceFile("in.ts", source, ts.ScriptTarget.ES2017, true, ts.ScriptKind.TS);
let nodes = 0;
const kinds = new Map();
(function walk(n) {
  nodes++;
  const k = ts.SyntaxKind[n.kind];
  kinds.set(k, (kinds.get(k) ?? 0) + 1);
  ts.forEachChild(n, walk);
})(sf);
console.log("ast-nodes", nodes);
console.log("ast-top-kinds", [...kinds.entries()].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1)).slice(0, 5).map(([k, v]) => k + ":" + v).join(","));
console.log("parse-diagnostics", sf.parseDiagnostics ? sf.parseDiagnostics.length : 0);

// 3. Type-check through a virtual compiler host: the checker, the most closure- and
//    Map-intensive part of the compiler.
const files = new Map([["main.ts", source + "\nconst bad: string = origin;\n"]]);
const host = {
  getSourceFile: (name, target) =>
    files.has(name) ? ts.createSourceFile(name, files.get(name), target, true) : undefined,
  writeFile: () => {},
  getDefaultLibFileName: () => "lib.d.ts",
  getCurrentDirectory: () => "/",
  getCanonicalFileName: (f) => f,
  useCaseSensitiveFileNames: () => true,
  getNewLine: () => "\n",
  fileExists: (f) => files.has(f),
  readFile: (f) => files.get(f),
};
const program = ts.createProgram(["main.ts"], { noLib: true, target: ts.ScriptTarget.ES2017 }, host);
const diags = ts.getPreEmitDiagnostics(program).filter((d) => d.file);
console.log("check-diagnostics", diags.length);
console.log("check-first", diags.length ? ts.flattenDiagnosticMessageText(diags[0].messageText, " ") : "none");
