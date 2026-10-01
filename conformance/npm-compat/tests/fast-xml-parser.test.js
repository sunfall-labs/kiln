// kiln eval smoke test (authored for kiln, not from porffor).
const { XMLParser, XMLValidator, XMLBuilder } = require('fast-xml-parser');

const parser = new XMLParser({ ignoreAttributes: false, attributeNamePrefix: '@_' });
const obj = parser.parse('<root><item id="1">one</item><item id="2">two</item></root>');
console.log('items:', JSON.stringify(obj.root.item));

const valid = XMLValidator.validate('<a><b/></a>');
console.log('valid xml:', valid === true);
const invalid = XMLValidator.validate('<a><b></a>');
console.log('invalid xml detected:', invalid !== true);

const builder = new XMLBuilder({ ignoreAttributes: false, attributeNamePrefix: '@_' });
const xml = builder.build({ note: { '@_id': '7', to: 'kiln', body: 'hello' } });
console.log('built:', xml);

const arr = parser.parse('<list><v>1</v><v>2</v><v>3</v></list>', true);
console.log('isArray:', Array.isArray(arr.list.v), 'values:', JSON.stringify(arr.list.v));

const cdata = parser.parse('<x><![CDATA[<raw>]]></x>');
console.log('cdata:', cdata.x);
