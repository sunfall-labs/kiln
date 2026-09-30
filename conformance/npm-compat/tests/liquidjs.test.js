// kiln eval smoke test (authored for kiln, not from porffor).
const { Liquid } = require('liquidjs');
const engine = new Liquid();
(async () => {
  console.log(await engine.parseAndRender('Hello {{ name }}!', { name: 'Ada' }));
  console.log(await engine.parseAndRender('{{ "a,b,c" | split: "," | last }}'));
  console.log(await engine.parseAndRender('{% if n > 2 %}big{% else %}small{% endif %}', { n: 5 }));
  console.log(await engine.parseAndRender('{% for i in (1..3) %}{{ i }}{% endfor %}'));
  console.log(await engine.parseAndRender('{{ "hi" | upcase }}'));
})();
