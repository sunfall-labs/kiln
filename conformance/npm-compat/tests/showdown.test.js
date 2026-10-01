// kiln eval smoke test (authored for kiln, not from porffor).
const showdown = require('showdown');
const c = new showdown.Converter();
console.log(JSON.stringify(c.makeHtml('# Hi')));
console.log(JSON.stringify(c.makeHtml('**bold** and *it*')));
console.log(JSON.stringify(c.makeHtml('- a\n- b')));
console.log(JSON.stringify(c.makeHtml('[x](https://x.com)')));
console.log(JSON.stringify(c.makeHtml('`code`')));
