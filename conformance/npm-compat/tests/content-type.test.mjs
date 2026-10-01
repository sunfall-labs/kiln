import { parse, format } from 'content-type';

console.log('--- parse string with multiple parameters ---');
const p1 = parse('TEXT/HTML; Charset="UTF-8"; foo=bar');
console.log('p1 type:', p1.type);
console.log('p1 parameters:', JSON.stringify(p1.parameters));

console.log('--- parse options: start offset & parameters: false ---');
const rawHeader = 'prefix: application/json; charset=utf-8';
const pOffset = parse(rawHeader, { start: 8 });
console.log('pOffset type:', pOffset.type);
console.log('pOffset charset:', pOffset.parameters.charset);

const pNoParams = parse('text/csv; header=present; delimiter=comma', { parameters: false });
console.log('pNoParams type:', pNoParams.type);
console.log('pNoParams parameters:', JSON.stringify(pNoParams.parameters));

console.log('--- format with quoted and escaped parameters ---');
const f1 = format({
  type: 'image/svg+xml',
  parameters: { name: 'my icon.svg', q: '0.8' }
});
console.log('f1:', f1);

console.log('--- format without parameters ---');
console.log('f2:', format({ type: 'application/json' }));

console.log('--- Error Handling ---');
try { parse(null); } catch (e) { console.log('caught null'); }
try { format({}); } catch (e) { console.log('caught missing type'); }
try { format({ type: 'text/html', parameters: { 'bad key!': 'val' } }); } catch (e) { console.log('caught invalid param name'); }
