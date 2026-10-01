// kiln eval smoke test (authored for kiln, not from porffor).
import mime from 'mime';
console.log(mime.getType('file.html'));
console.log(mime.getType('photo.PNG'));
console.log(mime.getExtension('application/json'));
console.log(mime.getType('no-extension'));
console.log(mime.getType('archive.tar.gz'));
console.log(mime.getExtension('text/html'));
