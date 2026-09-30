// kiln eval smoke test (authored for kiln, not from porffor).
const mime = require('mime-types');
console.log(mime.lookup('file.html'));
console.log(mime.lookup('noext'));
console.log(mime.extension('application/json'));
console.log(mime.contentType('json'));
console.log(mime.charset('text/html'));
console.log(mime.lookup('archive.tar.gz'));
