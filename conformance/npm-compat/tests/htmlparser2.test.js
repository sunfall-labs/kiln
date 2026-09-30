// kiln eval smoke test (authored for kiln, not from porffor).
const { parseDocument, DomUtils } = require('htmlparser2');

const doc = parseDocument('<div id="a"><p class="x">Hello <b>world</b></p><p>second</p></div>');
const ps = DomUtils.getElementsByTagName('p', doc);
console.log('p count:', ps.length);
console.log('text:', DomUtils.textContent(ps[0]));
console.log('by id:', DomUtils.getElementById('a', doc).tagName);

const fragDoc = parseDocument('<ul><li>one</li><li>two</li></ul>');
console.log('fragment li count:', DomUtils.getElementsByTagName('li', fragDoc).length);

const withAttrs = parseDocument('<a href="https://example.com" title="t">link</a>');
const a = DomUtils.getElementsByTagName('a', withAttrs)[0];
console.log('attrs:', JSON.stringify(DomUtils.getAttributeValue(a, 'href')));

const xml = parseDocument('<root><item>1</item></root>', { xmlMode: true });
console.log('xmlMode:', DomUtils.getElementsByTagName('item', xml).length === 1);

const empty = parseDocument('');
console.log('empty children:', empty.children.length);
console.log('has class:', DomUtils.hasAttrib(ps[0], 'class'));
