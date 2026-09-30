// kiln eval smoke test (authored for kiln, not from porffor).
const Mustache = require('mustache');
console.log(Mustache.render('Hello {{name}}!', {name: 'Ada'}));
console.log(Mustache.render('{{#items}}[{{.}}]{{/items}}', {items: [1, 2, 3]}));
console.log(Mustache.render('{{^missing}}fallback{{/missing}}', {}));
console.log(Mustache.render('{{! comment }}ok', {}));
console.log(Mustache.render('{{a}}', {a: '<tag>'}));
