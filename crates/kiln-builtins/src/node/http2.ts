// Deterministic node:http2 implementation (Phase 5.2, ADR 0005)

export function createServer(): never {
  throw new Error('node:http2 requires an HTTP/2 host capability that this realm does not provide');
}

export function createSecureServer(): never {
  throw new Error('node:http2 requires an HTTP/2 host capability that this realm does not provide');
}

const http2Default = {
  createSecureServer,
  createServer,
};

export default http2Default;
