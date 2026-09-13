// kiln-builtins: node:https implementation (Phase 5.2, ADR 0005)

export function createServer(..._args: any[]): never {
  throw new Error("node:https requires a TLS host capability that this realm does not provide");
}

const httpsNamespace = {
  createServer,
  default: null as any,
};
httpsNamespace.default = httpsNamespace;

export default httpsNamespace;
