// PostCSS: a CSS tokenizer, parser, mutable AST and stringifier.
//
// A second parser toolchain with a very different construction from acorn's and babel's: a
// hand-rolled character-code tokenizer, an AST of ES classes with getters, `Symbol.iterator` on
// containers, per-node source maps, and a visitor plugin protocol driven by a lazily-built
// dispatch table. It also leans on `String.prototype` scanning far more than on regexes.
import postcss from "postcss";

const css = `
/* header */
@import url("a.css");
@media (min-width: 700px) and (orientation: landscape) {
  .a, .b > .c ~ .d { color: #fff; margin: 0 auto !important; }
}
:root { --brand: hsl(210 50% 40% / .5); }
.e::before { content: "a\\"b"; background: url(data:image/png;base64,AA==) no-repeat; }
@supports (display: grid) { .f { display: grid; grid-template: "a b" 1fr / auto 1fr; } }
@font-face { font-family: "X"; src: local("X"), url(x.woff2) format("woff2"); }
`;

const root = postcss.parse(css, { from: "in.css" });
let nodes = 0;
const byType = new Map();
root.walk((node) => {
  nodes++;
  byType.set(node.type, (byType.get(node.type) ?? 0) + 1);
});
console.log("nodes", nodes);
console.log("types", [...byType.entries()].sort((a, b) => (a[0] < b[0] ? -1 : 1)).map(([k, v]) => k + ":" + v).join(","));
const firstAt = root.nodes.find((n) => n.type === "atrule");
console.log("first-atrule", firstAt.type, firstAt.name, firstAt.params);

// Container iteration protocol.
console.log("root-children", root.nodes.length, "each-ok", (() => { let n = 0; root.each(() => n++); return n === root.nodes.length; })());

// Mutation, then re-stringify: the printer must round-trip raws it did not author.
root.walkDecls(/^color$/, (d) => { d.value = "red"; });
root.walkRules((r) => { if (r.selector.includes(".a")) r.selector = r.selector.replace(".a", ".renamed"); });
root.append(postcss.rule({ selector: ".added" }).append(postcss.decl({ prop: "top", value: "0" })));
const out = root.toString();
console.log("out-has-red", out.includes("color: red"), "out-has-renamed", out.includes(".renamed"), "out-has-added", out.includes(".added") && out.includes("top: 0"));
console.log("out-preserved-comment", out.includes("/* header */"), "out-important", out.includes("!important"));

// A plugin, run through the async processor: this is the path a real build takes.
const plugin = () => ({
  postcssPlugin: "count",
  Declaration(decl) { decl.raws.__seen = true; },
  Rule(rule) { rule.raws.__seen = true; },
});
plugin.postcss = true;

postcss([plugin()])
  .process(out, { from: "b.css" })
  .then((result) => {
    console.log("processed-len-stable", result.css.length === out.length);
    console.log("warnings", result.warnings().length, "messages", result.messages.length);

    // A syntax error must be a located CssSyntaxError, not a crash.
    try {
      postcss.parse(".a { color: ", { from: "bad.css" });
      console.log("syntax-error none");
    } catch (e) {
      console.log("syntax-error", e.name, e.line, e.column);
    }
  });
