console.log(atob(' YQ==\n'), atob('YWI'), btoa('ab\x00\xff'));
for (const input of ['Y', 'YQ=', 'YQ===', 'Y\vQ==', '$']) {
  try { atob(input); }
  catch (error) { console.log(error.name); }
}
try { btoa('\u0100'); } catch (error) { console.log(error.name); }
const bytes = new Uint8Array(9);
console.log(crypto.getRandomValues(bytes) === bytes, bytes.length);
const view = new Uint16Array(bytes.buffer, 2, 2);
console.log(crypto.getRandomValues(view) === view, view.byteLength);
try { crypto.getRandomValues(new Float32Array(1)); } catch (error) { console.log(error.name); }
try { crypto.getRandomValues(new DataView(new ArrayBuffer(1))); } catch (error) { console.log(error.name); }
try { crypto.getRandomValues(new Uint8Array(65537)); } catch (error) { console.log(error.name); }
console.log(typeof crypto.randomUUID(), crypto.randomUUID().length);
const hmacKey = await crypto.subtle.importKey(
  "raw", new TextEncoder().encode("key"), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]
);
const hmac = await crypto.subtle.sign("HMAC", hmacKey, new TextEncoder().encode("data"));
console.log(Buffer.from(hmac).toString("hex"),
  await crypto.subtle.verify("HMAC", hmacKey, hmac, new TextEncoder().encode("data")),
  await crypto.subtle.verify("HMAC", hmacKey, hmac, new TextEncoder().encode("other")));
