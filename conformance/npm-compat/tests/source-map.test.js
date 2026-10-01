// kiln eval smoke test (authored for kiln, not from porffor).
const {SourceMapGenerator, SourceNode} = require('source-map');
const gen = new SourceMapGenerator({file: 'out.js'});
gen.addMapping({generated: {line: 1, column: 0}, original: {line: 1, column: 0}, source: 'in.js', name: 'foo'});
gen.setSourceContent('in.js', 'const foo = 1;');
const map = JSON.parse(gen.toString());
console.log(map.file, map.sources.join(','), map.mappings);
const node = new SourceNode(1, 0, 'in.js', 'const foo = 1;');
console.log(JSON.stringify(node.toStringWithSourceMap().code));
