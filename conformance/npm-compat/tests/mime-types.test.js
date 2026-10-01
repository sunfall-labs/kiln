const mime = require('mime-types');

console.log('--- mime.lookup ---');
console.log(mime.lookup('file.html'));
console.log(mime.lookup('FILE.HTML'));
console.log(mime.lookup('document.pdf'));
console.log(mime.lookup('image.svg'));
console.log(mime.lookup('script.mjs'));
console.log(mime.lookup('module.wasm'));
console.log(mime.lookup('archive.tar.gz'));
console.log(mime.lookup('archive.tar.bz2'));
console.log(mime.lookup('path/to/nested/style.css'));
console.log(mime.lookup('.gitignore'));
console.log(mime.lookup('noext'));
console.log(mime.lookup('file.unknownextension'));
console.log(mime.lookup(''));
console.log(mime.lookup(null));
console.log(mime.lookup(undefined));
console.log(mime.lookup(123));

console.log('--- mime.contentType ---');
console.log(mime.contentType('html'));
console.log(mime.contentType('json'));
console.log(mime.contentType('text/markdown'));
console.log(mime.contentType('application/json'));
console.log(mime.contentType('image/png'));
console.log(mime.contentType('text/html; charset=iso-8859-1'));
console.log(mime.contentType('unknown'));
console.log(mime.contentType('invalid/type'));

console.log('--- mime.extension ---');
console.log(mime.extension('text/html'));
console.log(mime.extension('application/json'));
console.log(mime.extension('application/json; charset=utf-8'));
console.log(mime.extension('image/jpeg'));
console.log(mime.extension('application/octet-stream'));
console.log(mime.extension('application/wasm'));
console.log(mime.extension('invalid/unknown'));

console.log('--- mime.charset ---');
console.log(mime.charset('text/html'));
console.log(mime.charset('text/markdown'));
console.log(mime.charset('text/css'));
console.log(mime.charset('application/json'));
console.log(mime.charset('image/png'));
console.log(mime.charset('text/custom; charset=shift_jis'));
console.log(mime.charset('invalid/unknown'));

console.log('--- mime.types and mime.extensions dictionaries ---');
console.log(mime.types['html']);
console.log(mime.types['json']);
console.log(mime.types['wasm']);
console.log(Array.isArray(mime.extensions['text/html']));
console.log(mime.extensions['text/html'].includes('html'));
console.log(mime.extensions['application/json'].includes('json'));
