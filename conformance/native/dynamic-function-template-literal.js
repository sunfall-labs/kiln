// Template literals inside dynamic code: `${}` interpolation must run, with ToString (not `+`)
// semantics, nesting, escapes and tagged calls. Before this, the text `a${x}` came back verbatim.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("plain", "return `plain`");
run("interpolate", "return `a${a}b${b}c`", 1, "two");
run("expression", "return `${a + 1}-${a * 2}-${[a, b]}`", 3, 4);
run("adjacent", "return `${a}${b}`", "x", "y");
run("empty-substitution-edges", "return `${a}`", "only");
run("nested", "return `outer ${`inner ${a}`} end`", 5);
run("nested-deep", "return `1${`2${`3${a}`}`}`", "x");
run("object-literal-in-sub", "return `${{k: a}.k}|${(() => { return {z: 1}; })().z}`", 7);
run("braces-in-string", "return `${'}' + a + '{'}`", "q");
run("backtick-in-string", "return `${'`'}${a}`", "z");
run("dollar-not-sub", "return `$ ${a} $$ \\${a} $`", "v");
run("escapes", "return `tab\\there\\nnl\\u0041\\x42\\u{43}`");
run("newline-raw", "return `line1\nline2`");
run("crlf-normalised", "return `a\r\nb\rc`.length");
run("tostring-hint", "return `${a}`", { toString() { return "ts"; }, valueOf() { return "vo"; } });
run("plus-differs", "return a + ''", { toString() { return "ts"; }, valueOf() { return "vo"; } });
run("symbol-throws", "return `${a}`", Symbol("s"));
run("null-undefined", "return `${a} ${b}`", null, undefined);
run("array-object", "return `${[1, 2]} ${{}}`");
run("number-formats", "return `${0.1 + 0.2} ${-0} ${1e21} ${1 / 3}`");
run("this-and-args", "return `${this.k}${arguments[0]}`.length", "z");
run("side-effect-order", "var log = []; var f = (n) => (log.push(n), n); return [`${f(1)}${f(2)}${f(3)}`, log]");
run("in-call", "return [1, 2].map((n) => `<${n}>`)");
run("surrogate", "return `${a}`.length", "\ud800");
run("unterminated-sub", "return `${a`");
run("empty-sub", "return `${}`");
run("bad-escape-untagged", "return `\\u{110000}`");
run("bad-hex-untagged", "return `\\xZ`");
run("comment-in-sub", "return `${a /* } */ + 1}`", 1);
run("regex-in-sub", "return `${/}/.test('}')}`");
run("ternary-in-sub", "return `${a ? 'yes' : 'no'}`", 0);
run("sequence-in-sub", "return `${(1, 2)}`");
run("string-concat", "return 'x' + `${a}` + 'y'", 9);

// tagged templates
const tagCode = "function tag(s, ...v) { return JSON.stringify([s, s.raw, v, Array.isArray(s), Object.isFrozen(s), Object.isFrozen(s.raw)]); } ";
run("tagged", tagCode + "return tag`a${a}b${b}c`", 1, 2);
run("tagged-no-subs", tagCode + "return tag`only`");
run("tagged-raw-escapes", tagCode + "return tag`x\\ny\\u0041${a}\\t`", 1);
run("tagged-invalid-escape", "return ((s) => [s[0], s.raw[0], s.length])`\\unicode and \\u{55}`");
run("tagged-invalid-then-valid", "return ((s, v) => [s[0], s[1], s.raw[0], v])`\\xZ${a}ok`", 3);
run("tagged-member-this", "var o = { k: 'self', t(s, v) { return [this.k, s[0], v]; } }; return o.t`a${a}`", 8);
run("tagged-computed-member", "var o = { t(s) { return s.raw[0]; } }; return o['t']`r\\n`");
run("tagged-call-result", "var mk = () => (s, v) => s[0] + v; return mk()`p${a}`", 2);
run("tagged-chain", "var t = (s) => (s2) => s[0] + s2[0]; return t`a``b`");
run("string-raw", "return String.raw`C:\\dir\\${a}\\n`", "x");
run("tagged-new", "function T(s) { this.v = s[0]; } var mk = (s) => T; return new mk`q`(['w']).v");
run("raw-descriptor", "return ((s) => JSON.stringify(Object.getOwnPropertyDescriptor(s, 'raw')))`a`");
run("tagged-frozen-write", "return ((s) => { s[0] = 'z'; s.extra = 1; return [s[0], s.extra, Object.isFrozen(s)]; })`a`");
run("tagged-sub-order", "var log = []; var f = (n) => (log.push(n), n); var t = (s, ...v) => log.concat(v); return t`${f(1)}${f(2)}`");
run("tagged-undefined-tag", "return (void 0)`x`");
run("tagged-sub-throws", "return ((s) => s)`${(() => { throw new RangeError('r'); })()}`");

// indirect eval path
console.log("eval-interp", (0, eval)("`q is ${2 + 2}`"));
console.log("eval-tagged", (0, eval)("((s, ...v) => s.raw.join('|') + v.join(','))`a${1}b${2}c`"));
console.log("eval-nested", (0, eval)("`a${`b${'c'}`}d`"));
