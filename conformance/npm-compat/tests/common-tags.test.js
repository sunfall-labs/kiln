// kiln eval smoke test (authored for kiln, not from porffor).
const { html, stripIndent, oneLine, commaLists } = require('common-tags');
console.log(JSON.stringify(html`<div>${'x'}</div>`));
console.log(JSON.stringify(stripIndent`  a
    b`));
console.log(oneLine`a
  b`);
console.log(commaLists`red${'green'}blue`);
console.log(JSON.stringify(html`<ul>${['a', 'b']}</ul>`));
