// kiln eval smoke test (authored for kiln, not from porffor).
import { parse, parseFragment, serialize } from 'parse5';

const doc = parse('<!DOCTYPE html><html><head><title>T</title></head><body><p id="x">Hi <b>there</b></p></body></html>');
const html = serialize(doc);
console.log(html.includes('<p id="x">Hi <b>there</b></p>'));
const frag = parseFragment('<li>a</li><li>b</li>');
console.log(serialize(frag));
console.log(serialize(parse('plain text')).includes('plain text'));
console.log(parseFragment('<a href="/x">y</a>').childNodes[0].attrs[0].value);
