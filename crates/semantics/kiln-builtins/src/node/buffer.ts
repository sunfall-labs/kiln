// kiln-builtins: node:buffer implementation (Phase 5.2, ADR 0005)

const G = globalThis as any;
const B = G.Buffer;

export const Buffer = B;
export const alloc = B.alloc;
export const allocUnsafe = B.allocUnsafe || B.alloc;
export const allocUnsafeSlow = B.allocUnsafeSlow || B.alloc;
export const byteLength = B.byteLength;
export const concat = B.concat;
export const constants = {
  MAX_LENGTH: 4294967296,
  MAX_STRING_LENGTH: 536870888,
};
export const from = B.from;
export const isBuffer = B.isBuffer;

try {
  B.constants = constants;
} catch (_) {}

const bufferNamespace = {
  Buffer,
  alloc,
  allocUnsafe,
  allocUnsafeSlow,
  byteLength,
  concat,
  constants,
  default: null as any,
  from,
  isBuffer,
};
bufferNamespace.default = bufferNamespace;

export default bufferNamespace;
