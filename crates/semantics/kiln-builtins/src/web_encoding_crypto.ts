// Node's web globals over the shared byte codecs and realm randomness policy.
(function installWebEncodingCrypto() {
  function bytesOf(value: any): Uint8Array {
    if (value instanceof ArrayBuffer) return new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    }
    throw new TypeError('Expected an ArrayBuffer or ArrayBufferView');
  }
  const typedArrayTagGetter = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get;
  const webcryptoPrimitive = (globalThis as any).__kilnWebCryptoPrimitive;
  try { delete (globalThis as any).__kilnWebCryptoPrimitive; } catch (_) {}
  const OP = { SHA256: 1, HKDF: 2, AES_ENCRYPT: 3, AES_DECRYPT: 4, ED_GENERATE: 5, ED_PUBLIC: 6, ED_SIGN: 7, ED_VERIFY: 8, ED_VALIDATE: 9, HMAC_SIGN: 10, HMAC_VERIFY: 11, RANDOM_BYTES: 12 };
  function primitive(op: number, key?: Uint8Array, data?: Uint8Array, iv?: Uint8Array, aad?: Uint8Array, length = 0): Uint8Array {
    if (typeof webcryptoPrimitive !== 'function') throw new Error('WebCrypto provider is unavailable');
    return webcryptoPrimitive(op, key || new Uint8Array(0), data || new Uint8Array(0), iv || new Uint8Array(0), aad || new Uint8Array(0), length);
  }
  function provider(op: number, key?: Uint8Array, data?: Uint8Array, iv?: Uint8Array, aad?: Uint8Array, length = 0): Uint8Array {
    try { return primitive(op, key, data, iv, aad, length); } catch (error) { throw new DOMException(String(error), 'OperationError'); }
  }
  // The operating system's CSPRNG, at most 65,536 bytes a call, for `node:crypto`'s random
  // functions: they must never fall back to `Math.random`, which is not a cryptographic source.
  kilnPrivate.secureRandomBytes = (length: number): Uint8Array =>
    primitive(OP.RANDOM_BYTES, undefined, undefined, undefined, undefined, length);
  function exactBuffer(bytes: Uint8Array): ArrayBuffer { return bytes.slice().buffer as ArrayBuffer; }
  function algorithmName(value: any): string {
    const name = typeof value === 'string' ? value : value && value.name;
    if (typeof name !== 'string') return name;
    switch (name.toLowerCase()) {
      case 'aes-gcm': return 'AES-GCM';
      case 'hkdf': return 'HKDF';
      case 'hmac': return 'HMAC';
      case 'ed25519': return 'Ed25519';
      case 'sha-256': return 'SHA-256';
      default: return name;
    }
  }
  function hashName(value: any): string {
    const hash = value && typeof value === 'object' ? value.hash : undefined;
    return String(typeof hash === 'string' ? hash : hash && hash.name || '').toUpperCase().replace('-', '');
  }
  const keyRecords = new WeakMap<object, any>(), keyToken = {};
  class CryptoKey {
    constructor(token: any, value: any) { if (token !== keyToken) throw new TypeError('Illegal constructor'); keyRecords.set(this, value); }
    get type(): string { return keyRecords.get(this).type; }
    get extractable(): boolean { return keyRecords.get(this).extractable; }
    get algorithm(): any {
      const algorithm = keyRecords.get(this).algorithm;
      return algorithm.hash ? Object.assign({}, algorithm, { hash: Object.assign({}, algorithm.hash) }) : Object.assign({}, algorithm);
    }
    get usages(): string[] { return keyRecords.get(this).usages.slice(); }
    get [Symbol.toStringTag](): string { return 'CryptoKey'; }
  }
  Object.defineProperty(globalThis, 'CryptoKey', { value: CryptoKey, writable: true, configurable: true });
  function record(key: any, usage: string, name?: string): any {
    const value = key && (typeof key === 'object' || typeof key === 'function') ? keyRecords.get(key) : undefined;
    if (!value || (name && value.name !== name) || !value.usages.includes(usage)) throw new DOMException('Invalid key or usage', 'InvalidAccessError');
    return value;
  }
  function makeKey(value: any): any { return new CryptoKey(keyToken, value); }
  function usagesOf(usages: any, allowed: string[]): string[] {
    const input = [...usages];
    const result: string[] = [];
    for (let i = 0; i < input.length; i++) {
      const usage = String(input[i]);
      if (!allowed.includes(usage)) throw new DOMException('Invalid key usage', 'SyntaxError');
      if (!result.includes(usage)) result.push(usage);
    }
    return result;
  }
  function base64url(bytes: Uint8Array): string { return bytes.toBase64({ alphabet: 'base64url', omitPadding: true }); }
  const subtle: any = {
    async importKey(format: string, keyData: any, algorithm: any, extractable: boolean, requested: string[]): Promise<any> {
      const name = algorithmName(algorithm), raw = bytesOf(keyData).slice();
      let type = 'secret', allowed: string[], publicKey: Uint8Array | undefined;
      if (name === 'AES-GCM') { if (format !== 'raw') throw new DOMException('Unsupported format', 'NotSupportedError'); if (![16,24,32].includes(raw.length)) throw new DOMException('Invalid AES key length', 'DataError'); allowed = ['encrypt','decrypt']; }
      else if (name === 'HKDF') { if (format !== 'raw') throw new DOMException('Unsupported format', 'NotSupportedError'); if (extractable) throw new DOMException('HKDF keys are not extractable', 'SyntaxError'); allowed = ['deriveKey','deriveBits']; }
      else if (name === 'HMAC') { if (format !== 'raw' || hashName(algorithm) !== 'SHA256') throw new DOMException('Unsupported HMAC parameters', 'NotSupportedError'); allowed = ['sign','verify']; }
      else if (name === 'Ed25519') {
        if (format === 'raw') { if (raw.length !== 32) throw new DOMException('Invalid Ed25519 public key', 'DataError'); provider(OP.ED_VALIDATE,raw); type='public'; allowed=['verify']; publicKey=raw; }
        else if (format === 'pkcs8') { publicKey=provider(OP.ED_PUBLIC,raw); type='private'; allowed=['sign']; }
        else throw new DOMException('Unsupported format', 'NotSupportedError');
      } else throw new DOMException('Unsupported algorithm', 'NotSupportedError');
      const keyUsages = usagesOf(requested, allowed!); if (!keyUsages.length && type !== 'public') throw new DOMException('At least one usage is required','SyntaxError');
      const spec = name === 'AES-GCM' ? {name,length:raw.length*8} : name === 'HMAC' ? {name,hash:{name:'SHA-256'},length:raw.length*8} : {name};
      return makeKey({type,name,algorithm:spec,extractable:!!extractable,usages:keyUsages,bytes:raw,publicKey});
    },
    async generateKey(algorithm: any, extractable: boolean, requested: string[]): Promise<any> {
      if (algorithmName(algorithm)!=='Ed25519') throw new DOMException('Unsupported algorithm','NotSupportedError');
      const uses=usagesOf(requested,['sign','verify']); if(!uses.length) throw new DOMException('At least one usage is required','SyntaxError');
      const data=provider(OP.ED_GENERATE), n=(data[0]<<8)|data[1], priv=data.slice(2,2+n), pub=data.slice(2+n);
      return {publicKey:makeKey({type:'public',name:'Ed25519',algorithm:{name:'Ed25519'},extractable:true,usages:uses.filter(x=>x==='verify'),bytes:pub,publicKey:pub}),privateKey:makeKey({type:'private',name:'Ed25519',algorithm:{name:'Ed25519'},extractable:!!extractable,usages:uses.filter(x=>x==='sign'),bytes:priv,publicKey:pub})};
    },
    async deriveBits(algorithm:any,baseKey:any,length:number|null):Promise<ArrayBuffer>{
      return exactBuffer(deriveMaterial(algorithm, baseKey, length, 'deriveBits'));
    },
    async deriveKey(algorithm:any,baseKey:any,target:any,extractable:boolean,uses:string[]):Promise<any>{const n=algorithmName(target),bits=n==='AES-GCM'?Number(target.length):n==='HMAC'?Number(target.length||256):0;if(!bits||bits%8)throw new DOMException('Unsupported key algorithm','NotSupportedError');const raw=deriveMaterial(algorithm,baseKey,bits,'deriveKey');return subtle.importKey('raw',exactBuffer(raw),target,extractable,uses);},
    async encrypt(algorithm:any,key:any,data:any):Promise<ArrayBuffer>{const v=record(key,'encrypt','AES-GCM');if(algorithmName(algorithm)!=='AES-GCM')throw new DOMException('Unsupported algorithm','NotSupportedError');if((algorithm.tagLength===undefined?128:Number(algorithm.tagLength))!==128)throw new DOMException('Unsupported tag length','NotSupportedError');return exactBuffer(provider(OP.AES_ENCRYPT,v.bytes,bytesOf(data),bytesOf(algorithm.iv),algorithm.additionalData===undefined?undefined:bytesOf(algorithm.additionalData)));},
    async decrypt(algorithm:any,key:any,data:any):Promise<ArrayBuffer>{const v=record(key,'decrypt','AES-GCM');if(algorithmName(algorithm)!=='AES-GCM')throw new DOMException('Unsupported algorithm','NotSupportedError');if((algorithm.tagLength===undefined?128:Number(algorithm.tagLength))!==128)throw new DOMException('Unsupported tag length','NotSupportedError');return exactBuffer(provider(OP.AES_DECRYPT,v.bytes,bytesOf(data),bytesOf(algorithm.iv),algorithm.additionalData===undefined?undefined:bytesOf(algorithm.additionalData)));},
    async sign(algorithm:any,key:any,data:any):Promise<ArrayBuffer>{const n=algorithmName(algorithm),v=record(key,'sign',n==='Ed25519'?'Ed25519':'HMAC'),op=n==='Ed25519'?OP.ED_SIGN:n==='HMAC'?OP.HMAC_SIGN:0;if(!op)throw new DOMException('Unsupported algorithm','NotSupportedError');return exactBuffer(provider(op,v.bytes,bytesOf(data)));},
    async verify(algorithm:any,key:any,signature:any,data:any):Promise<boolean>{const n=algorithmName(algorithm),v=record(key,'verify',n==='Ed25519'?'Ed25519':'HMAC'),op=n==='Ed25519'?OP.ED_VERIFY:n==='HMAC'?OP.HMAC_VERIFY:0;if(!op)throw new DOMException('Unsupported algorithm','NotSupportedError');return provider(op,n==='Ed25519'?v.publicKey:v.bytes,bytesOf(data),bytesOf(signature))[0]===1;},
    async exportKey(format:string,key:any):Promise<any>{const v=key&&keyRecords.get(key);if(!v)throw new DOMException('Invalid CryptoKey','InvalidAccessError');if(!v.extractable)throw new DOMException('Key is not extractable','InvalidAccessError');if(format==='raw'&&v.name==='Ed25519'&&v.type==='public')return exactBuffer(v.publicKey);if(format==='raw'&&v.type==='secret')return exactBuffer(v.bytes);if(format==='pkcs8'&&v.name==='Ed25519'&&v.type==='private')return exactBuffer(v.bytes);if(format==='jwk'&&v.name==='Ed25519'){const j:any={kty:'OKP',crv:'Ed25519',x:base64url(v.publicKey),key_ops:v.usages.slice(),ext:true};if(v.type==='private')j.d=base64url(v.bytes.slice(-32));return j;}if(format==='jwk'&&v.type==='secret')return{kty:'oct',k:base64url(v.bytes),alg:v.name==='AES-GCM'?`A${v.bytes.length*8}GCM`:'HS256',key_ops:v.usages.slice(),ext:true};throw new DOMException('Unsupported export','NotSupportedError');},
    async digest(algorithm:any,data:any):Promise<ArrayBuffer>{if(String(algorithmName(algorithm)||'').toUpperCase().replace('-','')!=='SHA256')throw new DOMException('Unsupported digest','NotSupportedError');return exactBuffer(provider(OP.SHA256,undefined,bytesOf(data)));},
  };
  function deriveMaterial(algorithm: any, baseKey: any, length: number | null, usage: string): Uint8Array {
    const value = record(baseKey, usage, 'HKDF');
    if (algorithmName(algorithm) !== 'HKDF' || hashName(algorithm) !== 'SHA256') {
      throw new DOMException('Unsupported derivation', 'NotSupportedError');
    }
    const bits = Number(length);
    if (!Number.isFinite(bits) || bits < 0 || bits % 8 !== 0 || bits > 65280) {
      throw new DOMException('Invalid bit length', 'OperationError');
    }
    return provider(OP.HKDF, value.bytes, bytesOf(algorithm.salt), bytesOf(algorithm.info), undefined, bits / 8);
  }
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
        const kind = view && typedArrayTagGetter ? Reflect.apply(typedArrayTagGetter, view, []) : undefined;
        if (!ArrayBuffer.isView(view) || kind === undefined ||
            kind === 'Float32Array' || kind === 'Float64Array' || kind === 'Float16Array') {
          throw new DOMException('Expected an integer TypedArray', 'TypeMismatchError');
        }
        if (view.byteLength > 65536) {
          throw new DOMException('The requested length exceeds 65,536 bytes', 'QuotaExceededError');
        }
        const bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
        bytes.set(provider(OP.RANDOM_BYTES, undefined, undefined, undefined, undefined, bytes.length));
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
