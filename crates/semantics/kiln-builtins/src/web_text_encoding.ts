// Web IDL receiver checks and bounded UTF-8 writes over the existing byte codec.
(function installTextEncoder() {
  const RawEncoder = TextEncoder;
  const encoders = new WeakMap<object, any>();
  const apply = Reflect.apply;
  const tag = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag
  )!.get!;
  const set = Uint8Array.prototype.set;
  const empty = new Uint8Array(0);
  function receiver(value: any): any {
    const raw = encoders.get(value);
    if (!raw) throw new TypeError('Illegal invocation');
    return raw;
  }
  class SharedTextEncoder {
    constructor() { encoders.set(this, new RawEncoder()); }
    get encoding(): string { receiver(this); return 'utf-8'; }
    encode(input: any = ''): Uint8Array {
      const raw = receiver(this);
      // Template conversion performs abstract ToString, including Symbol rejection.
      return apply(raw.encode, raw, [`${input}`]);
    }
    encodeInto(source: any, destination: Uint8Array): { read: number; written: number } {
      receiver(this);
      if (arguments.length < 2) throw new TypeError('encodeInto requires two arguments');
      const input = `${source}`;
      if (apply(tag, destination, []) !== 'Uint8Array') {
        throw new TypeError('Expected a Uint8Array destination');
      }
      // Validate detached buffers even when the source or destination is empty.
      apply(set, destination, [empty]);
      let read = 0, written = 0;
      while (read < input.length) {
        let point = input.charCodeAt(read), units = 1;
        if (point >= 0xd800 && point <= 0xdbff) {
          const low = input.charCodeAt(read + 1);
          if (low >= 0xdc00 && low <= 0xdfff) {
            point = 0x10000 + ((point - 0xd800) << 10) + low - 0xdc00;
            units = 2;
          } else point = 0xfffd;
        } else if (point >= 0xdc00 && point <= 0xdfff) point = 0xfffd;
        const count = point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
        if (written + count > destination.length) break;
        if (count === 1) destination[written++] = point;
        else {
          if (count === 2) destination[written++] = 0xc0 | (point >>> 6);
          else {
            if (count === 4) {
              destination[written++] = 0xf0 | (point >>> 18);
              destination[written++] = 0x80 | ((point >>> 12) & 0x3f);
            } else destination[written++] = 0xe0 | (point >>> 12);
            destination[written++] = 0x80 | ((point >>> 6) & 0x3f);
          }
          destination[written++] = 0x80 | (point & 0x3f);
        }
        read += units;
      }
      return { read, written };
    }
  }
  for (const key of ['encoding', 'encode', 'encodeInto']) {
    const descriptor = Object.getOwnPropertyDescriptor(SharedTextEncoder.prototype, key)!;
    descriptor.enumerable = true;
    Object.defineProperty(SharedTextEncoder.prototype, key, descriptor);
  }
  Object.defineProperty(SharedTextEncoder, 'name', { value: 'TextEncoder', configurable: true });
  Object.defineProperty(SharedTextEncoder.prototype, Symbol.toStringTag, {
    value: 'TextEncoder', configurable: true
  });
  Object.defineProperty(globalThis, 'TextEncoder', {
    value: SharedTextEncoder, writable: true, configurable: true
  });
})();
