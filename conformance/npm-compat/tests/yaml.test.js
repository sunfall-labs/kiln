// kiln npm functional test: yaml
const YAML = require('yaml');

// 1. Multi-document YAML streams
const streamYaml = '--- \nname: doc1\ncount: 10\n--- \nname: doc2\ncount: 20\n';
const docs = YAML.parseAllDocuments(streamYaml);
console.log('doc-count:', docs.length);
console.log('doc-names:', JSON.stringify(docs.map(d => d.toJS().name)));

// 2. Anchors and Aliases with Merge Keys
const anchorYaml = [
  'defaults: &base',
  '  timeout: 30',
  '  retries: 3',
  'production:',
  '  <<: *base',
  '  retries: 5',
  '  cluster: us-east'
].join('\n');
const parsedAnchors = YAML.parse(anchorYaml, { merge: true });
console.log('production-config:', JSON.stringify(parsedAnchors.production));

// 3. Document AST mutation & serialization
const doc = new YAML.Document({ title: 'Kiln', version: 1 });
doc.set('description', 'Native incremental compiler');
doc.setIn(['metadata', 'authors'], ['Andrew']);
doc.addIn(['metadata', 'authors'], 'Community');
console.log('doc-authors:', JSON.stringify(doc.getIn(['metadata', 'authors']).toJS(doc)));
const stringified = doc.toString().trim().split('\n').join(' | ');
console.log('doc-stringified:', stringified);

// 4. Block scalar formatting: literal (|) vs folded (>)
const blocksYaml = [
  'literal: |',
  '  line 1',
  '  line 2',
  'folded: >',
  '  line 1',
  '  line 2'
].join('\n');
const parsedBlocks = YAML.parse(blocksYaml);
console.log('literal-has-newline:', parsedBlocks.literal.includes('\n'));
console.log('folded-is-single-line:', parsedBlocks.folded.trim() === 'line 1 line 2');

// 5. Error handling for malformed YAML
let parseErrorCaught = false;
try {
  YAML.parse('key: [unclosed list');
} catch (e) {
  parseErrorCaught = true;
}
console.log('parse-error-caught:', parseErrorCaught);
