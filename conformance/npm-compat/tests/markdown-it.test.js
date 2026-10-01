// kiln eval smoke test (authored for kiln, not from porffor).
const MarkdownIt = require('markdown-it');
const md = new MarkdownIt();
console.log(md.render('# Title\n\n**bold** and *italic* and `code`').trim());
console.log(md.render('- a\n- b\n').trim());
console.log(md.render('[link](https://example.com)').trim());
console.log(md.renderInline('**x**'));
console.log(md.render('> quote').trim());
