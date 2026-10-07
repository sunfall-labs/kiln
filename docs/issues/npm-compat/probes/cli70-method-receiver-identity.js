function response() {
  return {
    headers: {},
    getHeader(name) { return this.headers[name]; },
    setHeader(name, value) {
      console.log('inside', this === globalThis.probeCurrent, Object.keys(this).join(','), Object.hasOwn(this, 'headers'), typeof this.headers, typeof Reflect.get(this, 'headers'));
      this.headers[name] = value;
    },
  };
}
for (let i = 0; i < 3; i++) {
  const r = response();
  globalThis.probeCurrent = r;
  const desc = Object.getOwnPropertyDescriptor(r, 'headers');
  console.log('outside', i, Object.keys(r).join(','), Object.hasOwn(r, 'headers'), typeof r.headers, typeof desc.value);
  r.setHeader('x', i);
  console.log('after', i, r.getHeader('x'));
}
