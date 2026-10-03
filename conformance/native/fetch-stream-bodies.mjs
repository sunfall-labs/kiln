const encode = (text) => new TextEncoder().encode(text);
function bytesStream(chunks) {
  let index = 0;
  return new ReadableStream({
    pull(controller) {
      if (index < chunks.length) controller.enqueue(chunks[index++]);
      else controller.close();
    }
  }, { highWaterMark: 0 });
}
async function rejectsTypeError(promise) {
  try { await promise; return false; }
  catch (error) { return error instanceof TypeError; }
}
async function main() {
  const source = bytesStream([
    new Uint8Array([0xc3]), new Uint8Array([0xa9, 0xf0, 0x9f]), new Uint8Array([0x98, 0x80])
  ]);
  const response = new Response(source);
  console.log('identity', response.body === source, response.bodyUsed);
  const reader = source.getReader();
  console.log('locked', response.bodyUsed, await rejectsTypeError(response.text()));
  reader.releaseLock();
  console.log('text', await response.text(), response.bodyUsed, response.body.locked);
  console.log('repeat', await rejectsTypeError(response.text()));

  const original = new Response(bytesStream([encode('first'), encode('second')]));
  const clone = original.clone();
  console.log('clone-unused', original.bodyUsed, clone.bodyUsed);
  console.log('clone', await original.text(), await clone.text());
  const direct = new Response(bytesStream([encode('direct')]));
  const directReader = direct.body.getReader();
  await directReader.read();
  directReader.releaseLock();
  console.log('direct-read', direct.bodyUsed, await rejectsTypeError(direct.text()));
  const invalid = new Response(bytesStream(['wrong chunk']));
  console.log('invalid', await rejectsTypeError(invalid.text()));
  const cancelled = new Response(bytesStream([encode('cancelled')]));
  await cancelled.body.cancel();
  console.log('cancelled', cancelled.bodyUsed, await rejectsTypeError(cancelled.text()));
  const request = new Request('http://localhost/', {
    method: 'POST', duplex: 'half', body: bytesStream([encode('{"value":'), encode('3}')])
  });
  const requestClone = request.clone();
  console.log('json', (await request.json()).value, (await requestClone.json()).value, request.duplex);
  const duplexGetter = Object.getOwnPropertyDescriptor(Request.prototype, 'duplex').get;
  console.log('duplex-brand', await rejectsTypeError(Promise.resolve().then(() => duplexGetter.call({}))));
  console.log('duplex-required', await rejectsTypeError(Promise.resolve().then(() =>
    new Request('http://localhost/', { method: 'POST', body: bytesStream([]) }))));
  const moved = new Request('http://localhost/', {
    method: 'POST', duplex: 'half', body: bytesStream([encode('moved')])
  });
  const destination = new Request(moved);
  console.log('transfer', moved.bodyUsed, await destination.text(), await rejectsTypeError(moved.text()));
  const empty = new Response(null);
  console.log('empty', await empty.text() === '', await empty.text() === '', empty.bodyUsed);
  const lazy = new Response('lazy');
  console.log('lazy', await lazy.text(), lazy.body.locked, lazy.bodyUsed);
}
main().catch((error) => { console.error(error); process.exitCode = 1; });
