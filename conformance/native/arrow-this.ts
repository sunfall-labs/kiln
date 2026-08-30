// An arrow snapshots `this` where it is created. Supplying another receiver through a direct
// call, `call`, `apply`, `bind`, or an array callback must not replace it.
const owner = {
  tag: "owner",
  make() {
    return () => this.tag;
  },
  makeAsync() {
    return async () => {
      await Promise.resolve(0);
      return this.tag;
    };
  },
};
const arrow = owner.make();
const wrong = { tag: "wrong" };
console.log(arrow(), arrow.call(wrong), arrow.apply(wrong, []), arrow.bind(wrong)());
console.log([0, 1].map(arrow).join(","));

// Module top-level `this` is undefined, and a top-level arrow captures that exact value.
const top = () => this;
console.log(top(), top.call(wrong));

// Async arrows use the generator frame machinery, but the lexical receiver rule is identical.
owner.makeAsync().call(wrong).then((value: string) => console.log(value));
