class Counter { #n=40; inc() { return ++this.#n; } } const c=new Counter(); console.log(JSON.stringify([c.inc(),c.inc()]));
