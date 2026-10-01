// kiln eval smoke test (authored for kiln, not from porffor).
const fresh = require('fresh');
console.log(fresh({'if-none-match': '"abc"'}, {etag: '"abc"'}));
console.log(fresh({'if-none-match': '"xyz"'}, {etag: '"abc"'}));
console.log(fresh({}, {}));
const d = new Date('2024-01-01T00:00:00Z').toUTCString();
console.log(fresh({'if-modified-since': d}, {'last-modified': d}));
console.log(fresh({'if-none-match': '*'}, {etag: '"abc"'}));
