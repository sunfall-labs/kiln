// kiln eval smoke test (authored for kiln, not from porffor).
const dedent = require('dedent');
console.log(JSON.stringify(dedent`
  hello
    world
`));
console.log(JSON.stringify(dedent`no indent`));
console.log(JSON.stringify(dedent`  a
  b
  c`));
const name = 'kiln';
console.log(JSON.stringify(dedent`
  hi ${name}
`));
