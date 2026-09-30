// kiln eval smoke test (authored for kiln, not from porffor).
const nunjucks = require('nunjucks');
console.log(nunjucks.renderString('Hello {{ name }}!', { name: 'Ada' }));
console.log(nunjucks.renderString('{% for i in items %}{{ i }};{% endfor %}', { items: [1, 2, 3] }));
console.log(nunjucks.renderString('{{ x | upper }}', { x: 'hi' }));
console.log(nunjucks.renderString('{% if n > 2 %}big{% else %}small{% endif %}', { n: 5 }));
console.log(nunjucks.renderString('{{ a | default("dflt") }}', {}));
