async function rejectsTypeError(promise) {
  try { await promise; return false; } catch (error) { return error instanceof TypeError; }
}
async function main() {
  const form = new FormData();
  form.append('field', 'é😀');
  form.append('field', 'second');
  form.set('line', 'one\ntwo\rthree\r\nfour');
  form.append('binary', new Blob([new Uint8Array([0, 128, 255])], { type: 'application/custom' }), 'bytes.bin');
  const original = new Response(form);
  const clone = original.clone();
  console.log('type', original.headers.get('content-type').startsWith('multipart/form-data; boundary='));
  form.set('field', 'mutated');
  const parsed = await original.formData();
  const cloned = await clone.formData();
  console.log('fields', parsed.getAll('field').join('|'), cloned.getAll('field').join('|'));
  console.log('lines', JSON.stringify(parsed.get('line')));
  const file = parsed.get('binary');
  console.log('file', file instanceof File, file.name, file.type, Array.from(await file.bytes()).join(','));
  console.log('used', original.bodyUsed, await rejectsTypeError(original.formData()));
  const request = new Request('http://localhost/', { method: 'POST', body: form });
  console.log('request', (await request.formData()).get('field'), request.bodyUsed);
  const urlencoded = new Response('x=a+b&x=%C3%A9&bad=%FF', { headers: { 'content-type': 'Application/X-WWW-Form-Urlencoded; charset=utf-8' } });
  const encoded = await urlencoded.formData();
  console.log('urlencoded', encoded.getAll('x').join('|'), encoded.get('bad'));
  const empty = new Response(new FormData());
  console.log('empty', Array.from((await empty.formData()).keys()).length);
  console.log('invalid', await rejectsTypeError(new Response('broken', { headers: { 'content-type': 'multipart/form-data' } }).formData()), await rejectsTypeError(new Response('text').formData()));
  const raw = '--edge\r\nContent-Disposition: form-data; name="file"; filename="a;b.bin"\r\nContent-Type: application/octet-stream\r\n\r\ninside\r\n--edge--\r\n';
  const rawForm = await new Response(raw, { headers: { 'content-type': 'multipart/form-data; boundary="edge"' } }).formData();
  console.log('quoted', rawForm.get('file').name, JSON.stringify(await rawForm.get('file').text()));
}
main().catch(error => { console.error(error); process.exitCode = 1; });
