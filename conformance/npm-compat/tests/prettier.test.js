// kiln eval smoke test (authored for kiln, not from porffor).
const prettier = require('prettier');
(async () => {
  console.log(JSON.stringify(await prettier.format('const x=1', {parser: 'babel'})));
  console.log(JSON.stringify(await prettier.format('{"a":1}', {parser: 'json'})));
  console.log((await prettier.getFileInfo('foo.js')).inferredParser);
  console.log(await prettier.check('const x = 1;\n', {parser: 'babel'}));
})();
