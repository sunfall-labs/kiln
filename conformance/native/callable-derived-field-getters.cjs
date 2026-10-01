class Callable {
  constructor(fn, opts) {
    return Object.assign(Object.setPrototypeOf(fn.bind(this), this.constructor.prototype), opts.attach);
  }
}
class Base extends Callable {
  attachments;
  constructor(attachments) { super(() => {}, { attach: attachments }); this.attachments = attachments; }
  get shallowMorphs() { return []; }
}
class Root extends Base { branches = this.kind === 'union' ? this.inner.branches : [this]; }
class Union extends Root {
  shallow = this.shallowMorphs;
  get shallowMorphs() { return this.branches.reduce((sum, item) => sum + item, 0); }
}
const instance = new Union({ kind: 'union', inner: { branches: [1, 2] } });
console.log(typeof instance, instance.kind, instance.branches.join(','), instance.shallow, instance instanceof Union);
Object.defineProperty(instance, 'ownAccessor', { get() { return this; } });
console.log(instance.ownAccessor === instance);
