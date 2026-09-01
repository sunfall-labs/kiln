const target = function (value) {
  return `target:${value}`;
};

const transparent = new Proxy(target, {});
console.log(transparent("one"));

const trapped = new Proxy(target, {
  apply(inner, receiver, args) {
    return `trap:${inner.call(receiver, args[0])}`;
  },
});
console.log(trapped("two"));
