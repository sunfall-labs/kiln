// A named class expression binds its own name inside its body, and nowhere else.
class Base {
  constructor(tag) {
    this.tag = tag;
  }
}

const Holder = class Inner extends Base {
  constructor() {
    super("inner");
  }
  // The name resolves from a method, through the closure machinery, like any capture.
  clone() {
    return new Inner();
  }
  static make() {
    return new Inner();
  }
};

console.log(Holder.make().tag, new Holder().clone().tag);
console.log(typeof Inner === "undefined" ? "not leaked" : "leaked");

// Anonymous class expressions still work, and two of them stay separate.
const A = class extends Base {
  constructor() {
    super("a");
  }
};
const B = class extends Base {
  constructor() {
    super("b");
  }
};
console.log(new A().tag, new B().tag);

// A named class *declaration* does leak its name, and can still see itself.
class Node {
  static of() {
    return new Node();
  }
}
console.log(Node.of() instanceof Node);
