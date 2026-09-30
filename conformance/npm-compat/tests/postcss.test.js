// kiln eval smoke test (authored for kiln, not from porffor).
const postcss = require('postcss');

postcss([
  (root) => {
    root.walkDecls((d) => {
      if (d.prop === 'color') d.value = 'blue';
    });
  },
])
  .process('a{color:red}', { from: undefined })
  .then((result) => {
    console.log(result.css);
    console.log(result.root.nodes[0].type);
  });

const decl = postcss.decl({ prop: 'margin', value: '0' });
console.log(decl.toString());
console.log(JSON.stringify(postcss.list.space('1px 2px')));
console.log(postcss.rule({ selector: 'p' }).toString());
