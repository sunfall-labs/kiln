// Web standards globals and operations under node host.

// 1. Global existence and types
console.log('fetch is function:', typeof fetch === 'function');
console.log('Headers is function:', typeof Headers === 'function');
console.log('Request is function:', typeof Request === 'function');
console.log('Response is function:', typeof Response === 'function');
console.log('FormData is function:', typeof FormData === 'function');
console.log('Blob is function:', typeof Blob === 'function');
console.log('File is function:', typeof File === 'function');
console.log('DOMException is function:', typeof DOMException === 'function');

// 2. Headers
const h = new Headers([['Content-Type', 'text/plain'], ['X-Custom', 'val1']]);
h.append('x-custom', 'val2');
console.log('h.get content-type:', h.get('content-type'));
console.log('h.get X-CUSTOM:', h.get('X-CUSTOM'));
console.log('h.has content-type:', h.has('content-type'));
console.log('h.has non-existent:', h.has('non-existent'));
h.set('x-custom', 'new-val');
console.log('h.set x-custom:', h.get('x-custom'));
h.delete('x-custom');
console.log('h.delete x-custom:', h.has('x-custom'));
const entries = Array.from(h.entries());
console.log('h.entries count:', entries.length);
console.log('h.entries[0]:', entries[0][0], entries[0][1]);

// 3. Blob & File
const blob = new Blob(['hello', ' ', 'world'], { type: 'text/plain' });
console.log('blob size:', blob.size);
console.log('blob type:', blob.type);

const file = new File(['file contents'], 'test.txt', { type: 'text/plain' });
console.log('file name:', file.name);
console.log('file size:', file.size);
console.log('file type:', file.type);
console.log('file is Blob:', file instanceof Blob);

// 4. FormData
const fd = new FormData();
fd.append('key1', 'val1');
fd.append('key1', 'val2');
fd.set('key2', 'val3');
console.log('fd.get key1:', fd.get('key1'));
console.log('fd.getAll key1:', fd.getAll('key1').join(','));
console.log('fd.has key2:', fd.has('key2'));
console.log('fd.has key3:', fd.has('key3'));
fd.delete('key1');
console.log('fd.has key1 after delete:', fd.has('key1'));

// 5. DOMException
const domEx = new DOMException('Something failed', 'AbortError');
console.log('domEx name:', domEx.name);
console.log('domEx message:', domEx.message);
console.log('domEx is Error:', domEx instanceof Error);

// 6. Request & Response
const req = new Request('http://example.com/api', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: '{"foo":"bar"}'
});
console.log('req method:', req.method);
console.log('req url:', req.url);
console.log('req header:', req.headers.get('content-type'));

const res = new Response('{"status":"ok"}', {
  status: 200,
  statusText: 'OK',
  headers: { 'Content-Type': 'application/json' }
});
console.log('res status:', res.status);
console.log('res statusText:', res.statusText);
console.log('res ok:', res.ok);
console.log('res header:', res.headers.get('content-type'));

const cloned = res.clone();
console.log('cloned status:', cloned.status);
console.log('cloned ok:', cloned.ok);

// 7. Math unboxed / open-world specialization
function mathOps(a, b) {
  const maxVal = Math.max(a, b);
  const minVal = Math.min(a, b);
  const sq = Math.sqrt(maxVal);
  const fl = Math.floor(sq + minVal);
  const ab = Math.abs(-fl);
  return ab;
}
console.log('mathOps:', mathOps(16, 9));
