class E extends Error { constructor(m) { super(m); this.name = 'E'; this.tag = 'T' } }
const e = new E('boom');
console.log(e instanceof Error, e instanceof E, e.message, e.name, e.tag, String(e));
class F extends globalThis.Error {}
console.log(new F('g') instanceof Error, new F('g').message);
