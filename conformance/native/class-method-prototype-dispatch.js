class Scope {
  init() {
    return this.crawl();
  }

  crawl() {
    return `proto:${this.value}`;
  }
}

const first = new Scope();
first.value = 1;
console.log(first.init());

first.crawl = function () {
  return `own:${this.value}`;
};
console.log(first.init());

const second = new Scope();
second.value = 2;
console.log(second.init());
