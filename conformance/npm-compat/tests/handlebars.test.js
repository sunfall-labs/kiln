// kiln eval smoke test (authored for kiln, not from porffor).
const Handlebars = require('handlebars');
console.log(Handlebars.compile('Hello, {{name}}! You have {{count}} messages.')({ name: 'Ada', count: 3 }));
console.log(Handlebars.compile('{{#each items}}{{this}};{{/each}}')({ items: ['a', 'b', 'c'] }));
Handlebars.registerHelper('upper', (s) => s.toUpperCase());
console.log(Handlebars.compile('{{upper word}}')({ word: 'hi' }));
console.log(Handlebars.compile('{{#if ok}}yes{{else}}no{{/if}}')({ ok: false }));
console.log(Handlebars.compile('{{missing}}')({}));
