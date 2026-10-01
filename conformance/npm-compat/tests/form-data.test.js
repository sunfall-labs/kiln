// kiln eval smoke test (authored for kiln, not from porffor).
const FormData = require('form-data');
const form = new FormData();
form.append('name', 'Ada');
form.append('file', Buffer.from('data-bytes'), {filename: 'f.bin', contentType: 'application/octet-stream'});
const headers = form.getHeaders();
console.log(headers['content-type'].startsWith('multipart/form-data; boundary='));
console.log(typeof form.getBoundary());
form.getLength((err, len) => console.log(err ? 'err' : len > 0));
const f2 = new FormData();
f2.append('a', '1');
console.log(f2.hasKnownLength());
