// Incremental CompressionStream state survives writes, close and collection-visible allocations.
async function collect(stream) {
  const reader = stream.readable.getReader();
  const chunks = [];
  while (true) {
    const item = await reader.read();
    if (item.done) break;
    chunks.push(item.value);
  }
  let size = 0;
  for (const chunk of chunks) size += chunk.length;
  const result = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.length; }
  return result;
}
async function roundtrip(format) {
  const source = new TextEncoder().encode("incremental bytes survive boundaries ".repeat(32));
  const compressor = new CompressionStream(format);
  const packedPromise = collect(compressor);
  const writer = compressor.writable.getWriter();
  for (let offset = 0; offset < source.length; offset += 7) await writer.write(source.slice(offset, offset + 7));
  await writer.close();
  const packed = await packedPromise;

  const decompressor = new DecompressionStream(format);
  const outputPromise = collect(decompressor);
  const decoder = decompressor.writable.getWriter();
  for (let offset = 0; offset < packed.length; offset += 5) await decoder.write(packed.slice(offset, offset + 5));
  await decoder.close();
  const output = await outputPromise;
  let equal = output.length === source.length;
  for (let i = 0; equal && i < source.length; i++) equal = output[i] === source[i];
  console.log(format, equal, output.length, output[0], output[output.length - 1]);
}
for (const format of ["gzip", "deflate", "deflate-raw"]) await roundtrip(format);

async function roundtripBufferSource(chunk, expected) {
  const compressor = new CompressionStream("deflate-raw");
  const packedPromise = collect(compressor);
  const writer = compressor.writable.getWriter();
  await writer.write(chunk);
  await writer.close();
  const packed = await packedPromise;
  const decompressor = new DecompressionStream("deflate-raw");
  const outputPromise = collect(decompressor);
  const reader = decompressor.writable.getWriter();
  await reader.write(packed.buffer);
  await reader.close();
  const output = await outputPromise;
  const actual = Array.from(output);
  console.log("BufferSource", JSON.stringify(actual) === JSON.stringify(expected), actual.join(","));
}

const storage = Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8]);
await roundtripBufferSource(storage.buffer, [1, 2, 3, 4, 5, 6, 7, 8]);
await roundtripBufferSource(new DataView(storage.buffer, 2, 3), [3, 4, 5]);
await roundtripBufferSource(new Uint16Array(storage.buffer, 2, 2), [3, 4, 5, 6]);
try {
  const invalid = new CompressionStream("gzip");
  const writer = invalid.writable.getWriter();
  await writer.write({ buffer: storage.buffer, byteOffset: 0, byteLength: 8 });
} catch (error) { console.log("BufferSource brand", error.name); }

const shape = new CompressionStream({ toString() { return "gzip"; } });
const descriptor = Object.getOwnPropertyDescriptor(CompressionStream.prototype, "readable");
console.log("shape", Object.keys(shape).length, descriptor.enumerable, Object.prototype.toString.call(shape));
try { new CompressionStream("br"); } catch (error) { console.log("format", error.name); }
try { descriptor.get.call({}); } catch (error) { console.log("brand", error.name); }
