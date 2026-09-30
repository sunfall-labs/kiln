// Node's web globals over the shared byte codecs and realm randomness policy.
(function installWebEncodingCrypto() {
  function bytesOf(value: any): Uint8Array {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
  }
  function sha256(input: Uint8Array): Uint8Array {
    const constants = [
      0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
      0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
      0xe49b69c1, 0xefbe4784, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
      0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
      0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
      0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
      0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
      0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    const paddedLength = Math.ceil((input.length + 9) / 64) * 64;
    const padded = new Uint8Array(paddedLength);
    padded.set(input);
    padded[input.length] = 0x80;
    const bitLength = input.length * 8;
    const tail = new DataView(padded.buffer);
    tail.setUint32(paddedLength - 8, Math.floor(bitLength / 0x100000000));
    tail.setUint32(paddedLength - 4, bitLength >>> 0);
    const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    const w = new Int32Array(64);
    const rotr = (n: number, b: number) => (n >>> b) | (n << (32 - b));
    for (let block = 0; block < paddedLength; block += 64) {
      for (let i = 0; i < 16; i++) w[i] = tail.getInt32(block + i * 4);
      for (let i = 16; i < 64; i++) {
        const x = w[i - 15], y = w[i - 2];
        const s0 = rotr(x, 7) ^ rotr(x, 18) ^ (x >>> 3);
        const s1 = rotr(y, 17) ^ rotr(y, 19) ^ (y >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
      }
      let [a, b, c, d, e, f, g, z] = h;
      for (let i = 0; i < 64; i++) {
        const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
        const ch = (e & f) ^ (~e & g);
        const t1 = (z + s1 + ch + constants[i] + w[i]) | 0;
        const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
        const maj = (a & b) ^ (a & c) ^ (b & c);
        const t2 = (s0 + maj) | 0;
        z = g; g = f; f = e; e = (d + t1) | 0;
        d = c; c = b; b = a; a = (t1 + t2) | 0;
      }
      h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0;
      h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + z) | 0;
    }
    const result = new Uint8Array(32);
    const output = new DataView(result.buffer);
    for (let i = 0; i < 8; i++) output.setInt32(i * 4, h[i]);
    return result;
  }
  function hmacSha256(key: Uint8Array, data: Uint8Array): Uint8Array {
    let material = key.length > 64 ? sha256(key) : key;
    const inner = new Uint8Array(64 + data.length);
    const outer = new Uint8Array(64 + 32);
    for (let i = 0; i < 64; i++) {
      const byte = i < material.length ? material[i] : 0;
      inner[i] = byte ^ 0x36;
      outer[i] = byte ^ 0x5c;
    }
    inner.set(data, 64);
    outer.set(sha256(inner), 64);
    return sha256(outer);
  }
  const subtle = {
    async importKey(format: string, keyData: any, algorithm: any, extractable: boolean, usages: string[]): Promise<any> {
      const name = typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name;
      const hash = algorithm && typeof algorithm === 'object' && algorithm.hash;
      const hashName = typeof hash === 'string' ? hash : hash && hash.name;
      if (format !== 'raw' || name !== 'HMAC' || String(hashName || '').toUpperCase().replace('-', '') !== 'SHA256') {
        throw new DOMException('Unsupported key format or algorithm', 'NotSupportedError');
      }
      return { type: 'secret', algorithm: { name: 'HMAC', hash: { name: 'SHA-256' } },
        extractable: !!extractable, usages: usages.slice(), _key: bytesOf(keyData).slice(),
        [Symbol.toStringTag]: 'CryptoKey' };
    },
    async sign(algorithm: any, key: any, data: any): Promise<ArrayBuffer> {
      const name = typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name;
      if (name !== 'HMAC' || !key || key.type !== 'secret' || !key.usages.includes('sign')) {
        throw new DOMException('Invalid key or algorithm', 'InvalidAccessError');
      }
      return hmacSha256(key._key, bytesOf(data)).buffer as ArrayBuffer;
    },
    async verify(algorithm: any, key: any, signature: any, data: any): Promise<boolean> {
      const expected = new Uint8Array(await this.sign(algorithm, key, data));
      const actual = bytesOf(signature);
      if (actual.length !== expected.length) return false;
      let difference = 0;
      for (let i = 0; i < actual.length; i++) difference |= actual[i] ^ expected[i];
      return difference === 0;
    },
    async digest(algorithm: any, data: any): Promise<ArrayBuffer> {
      const name = typeof algorithm === 'string' ? algorithm : algorithm && algorithm.name;
      if (String(name || '').toUpperCase().replace('-', '') !== 'SHA256') {
        throw new DOMException('Unsupported digest algorithm', 'NotSupportedError');
      }
      return sha256(bytesOf(data)).buffer as ArrayBuffer;
    },
  };
  function invalidCharacter(): never {
    throw new DOMException('Invalid character', 'InvalidCharacterError');
  }
  function atob(data: any): string {
    if (arguments.length === 0) throw new TypeError('atob requires one argument');
    const value = `${data}`;
    let bytes: Uint8Array;
    try { bytes = Uint8Array.fromBase64(value); }
    catch (_) { return invalidCharacter(); }
    let result = '';
    for (let i = 0; i < bytes.length; i++) result += String.fromCharCode(bytes[i]);
    return result;
  }
  function btoa(data: any): string {
    if (arguments.length === 0) throw new TypeError('btoa requires one argument');
    const value = `${data}`;
    const bytes = new Uint8Array(value.length);
    for (let i = 0; i < value.length; i++) {
      const code = value.charCodeAt(i);
      if (code > 255) return invalidCharacter();
      bytes[i] = code;
    }
    return bytes.toBase64();
  }
  Object.defineProperty(globalThis, 'atob', { value: atob, writable: true, enumerable: true, configurable: true });
  Object.defineProperty(globalThis, 'btoa', { value: btoa, writable: true, enumerable: true, configurable: true });
  // Both engines use the same integer-view checks and realm randomness policy.
  {
    const crypto = {
      getRandomValues(view: any): any {
        const kind = Object.prototype.toString.call(view);
        if (!ArrayBuffer.isView(view) || kind === '[object DataView]' ||
            kind === '[object Float32Array]' || kind === '[object Float64Array]' ||
            kind === '[object Float16Array]') {
          throw new DOMException('Expected an integer TypedArray', 'TypeMismatchError');
        }
        if (view.byteLength > 65536) {
          throw new DOMException('The requested length exceeds 65,536 bytes', 'QuotaExceededError');
        }
        const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
        for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
        return view;
      },
      randomUUID(): string {
        const bytes = new Uint8Array(16);
        crypto.getRandomValues(bytes);
        bytes[6] = (bytes[6] & 15) | 64;
        bytes[8] = (bytes[8] & 63) | 128;
        let result = '';
        const alphabet = '0123456789abcdef';
        for (let i = 0; i < bytes.length; i++) {
          if (i === 4 || i === 6 || i === 8 || i === 10) result += '-';
          result += alphabet[bytes[i] >> 4] + alphabet[bytes[i] & 15];
        }
        return result;
      },
      subtle,
    };
    Object.defineProperty(globalThis, 'crypto', { value: crypto, writable: true, enumerable: true, configurable: true });
  }
})();
