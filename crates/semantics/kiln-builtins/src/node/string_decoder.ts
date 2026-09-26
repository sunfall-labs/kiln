// kiln-builtins: node:string_decoder implementation (Phase 5.2, ADR 0005)

function getIncompleteUtf8Length(bytes: Uint8Array): number {
  const len = bytes.length;
  if (len === 0) return 0;
  for (let i = 1; i <= Math.min(len, 3); i++) {
    const byte = bytes[len - i];
    if ((byte & 0xe0) === 0xc0) {
      return i < 2 ? i : 0;
    }
    if ((byte & 0xf0) === 0xe0) {
      return i < 3 ? i : 0;
    }
    if ((byte & 0xf8) === 0xf0) {
      return i < 4 ? i : 0;
    }
    if ((byte & 0xc0) !== 0x80) {
      break;
    }
  }
  return 0;
}

export class StringDecoder {
  encoding: string;
  private pending: Uint8Array;
  private decoder: TextDecoder;

  static StringDecoder: typeof StringDecoder;
  static default: typeof StringDecoder;

  constructor(encoding = 'utf8') {
    const norm = String(encoding).toLowerCase().replace(/[-_]/g, '');
    if (norm === 'utf8' || norm === 'utf-8') {
      this.encoding = 'utf8';
    } else {
      this.encoding = norm;
    }
    this.pending = new Uint8Array(0);
    this.decoder = new TextDecoder('utf-8');
  }

  write(buffer: any): string {
    if (!buffer) return '';
    const chunk = typeof buffer === 'string'
      ? new TextEncoder().encode(buffer)
      : (buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer));
    let bytes = chunk;
    if (this.pending.length > 0) {
      const combined = new Uint8Array(this.pending.length + chunk.length);
      combined.set(this.pending, 0);
      combined.set(chunk, this.pending.length);
      bytes = combined;
    }
    const incomplete = getIncompleteUtf8Length(bytes);
    if (incomplete > 0) {
      this.pending = bytes.slice(bytes.length - incomplete);
      const complete = bytes.slice(0, bytes.length - incomplete);
      return this.decoder.decode(complete);
    } else {
      this.pending = new Uint8Array(0);
      return this.decoder.decode(bytes);
    }
  }

  end(buffer?: any): string {
    let res = '';
    if (buffer) {
      res = this.write(buffer);
    }
    if (this.pending.length > 0) {
      res += '\uFFFD';
      this.pending = new Uint8Array(0);
    }
    return res;
  }
}

StringDecoder.StringDecoder = StringDecoder;
StringDecoder.default = StringDecoder;

const defaultExport = {
  StringDecoder,
  default: null as any,
};
defaultExport.default = defaultExport;

export default defaultExport;
