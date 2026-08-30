// A template converts every hole with ToString and concatenates, so the result is a string even
// when nothing in it is one -- `${1}${2}` is "12", not 3.
const a = 1, b = 2;
console.log(`${a}${b}`, `${a} + ${b} = ${a + b}`, `no holes`, ``, `${''}`);
console.log(`start ${a}`, `${a} end`, `${a}${''}${b}`);
console.log(`${[1, 2]}`, `${{}}`, `${null}`, `${undefined}`, `${true}`);
console.log(`${1e21}`, `${0.1 + 0.2}`, `${-0}`);
// Escapes and newlines are part of the cooked text.
console.log(`a\tb`, `line\nbreak`.length, `a\`b`, `\${not a hole}`);
// Nesting, and a hole whose expression is itself a template.
const name = 'world';
console.log(`hello ${`dear ${name}`}!`);
// Holes are evaluated left to right, once each.
const order = [];
const mark = (n) => { order.push(n); return n; };
console.log(`${mark(1)}${mark(2)}${mark(3)}`, order.join(','));
