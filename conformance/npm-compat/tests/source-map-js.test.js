// kiln eval smoke test (authored for kiln, not from porffor).
const { SourceMapConsumer, SourceNode } = require('source-map-js');

const rawMap = {
  version: 3,
  file: 'out.js',
  sources: ['src.js'],
  names: ['greet'],
  mappings: 'AAAA,SAASA,KAAT,CAAoB,MAAM',
};

const consumer = new SourceMapConsumer(rawMap);
const pos = consumer.originalPositionFor({ line: 1, column: 10 });
console.log(JSON.stringify([pos.source, pos.line, pos.column, pos.name]));

const node = new SourceNode(1, 0, 'src.js', ['const ', new SourceNode(1, 6, 'src.js', 'x'), ' = 1;']);
const { code, map } = node.toStringWithSourceMap({ file: 'out.js' });
console.log(code);
console.log(map.toJSON().sources);
