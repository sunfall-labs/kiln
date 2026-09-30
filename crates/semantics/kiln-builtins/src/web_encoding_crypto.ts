// Node's web globals over the shared byte codecs and realm randomness policy.
(function installWebEncodingCrypto() {
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
    };
    Object.defineProperty(globalThis, 'crypto', { value: crypto, writable: true, enumerable: true, configurable: true });
  }
})();
