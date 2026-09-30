// kiln eval smoke test (authored for kiln, not from porffor).
import pathIsAbsolute from 'path-is-absolute';

console.log('posix abs:', pathIsAbsolute('/foo/bar'));
console.log('posix rel:', pathIsAbsolute('foo/bar'));
console.log('posix dot:', pathIsAbsolute('./foo'));
console.log('win abs:', pathIsAbsolute('C:\\foo\\bar', { windows: true }));
console.log('win rel:', pathIsAbsolute('foo\\bar', { windows: true }));
console.log('win unc:', pathIsAbsolute('\\\\server\\share', { windows: true }));
console.log('empty:', pathIsAbsolute(''));
console.log('cwd-ish:', pathIsAbsolute('.'));
