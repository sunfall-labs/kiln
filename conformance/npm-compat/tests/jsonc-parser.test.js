// kiln eval smoke test (authored for kiln, not from porffor).
const { parse, parseTree, printParseErrorCode, getLocation, visit } = require('jsonc-parser');

const withComments = `{
  // a comment
  "name": "kiln",
  "n": 42, /* trailing */
  "list": [1, 2, 3,],
}`;
const errors = [];
const obj = parse(withComments, errors, { allowTrailingComma: true, disallowComments: false });
console.log('parse:', JSON.stringify(obj), 'errors:', errors.length);

const strictErrs = [];
parse('{"a": }', strictErrs);
console.log('error code:', strictErrs.length > 0 ? printParseErrorCode(strictErrs[0].error) : 'none');

const tree = parseTree('{"a": {"b": [1, 2]}}');
console.log('tree type:', tree.type, 'children:', tree.children.length);

const loc = getLocation('{"a": 1}', 5);
console.log('location path:', JSON.stringify(loc.path), 'isAtPropertyKey:', loc.isAtPropertyKey);

const visited = [];
visit('{"x": 1}', {
  onObjectProperty: (p) => visited.push('prop:' + p),
  onLiteralValue: (v) => visited.push('lit:' + v)
});
console.log('visited:', JSON.stringify(visited));
