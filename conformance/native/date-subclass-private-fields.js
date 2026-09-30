class Instant extends Date {
  #flag = true;
  constructor(value) { super(value); }
  flag() { return this.#flag; }
}
const date = new Instant('1979-05-27T07:32:00Z');
console.log(Object.prototype.toString.call(date), date instanceof Date, date instanceof Instant);
console.log(date.getTime(), Date.prototype.getTime.call(date), date.flag());
console.log(date.toISOString());

const marker = Symbol('marker');
const held = { value: 42 };
const date = new Date(0);
date.extra = held;
date[marker] = 'symbol';
console.log(Object.keys(date).join(','), Object.hasOwn(date, 'extra'),
  Object.getOwnPropertyDescriptor(date, marker).value);
for (let i = 0; i < 50; i++) new Date(i);
console.log(date.extra.value, date.getTime(), delete date.extra, Object.hasOwn(date, 'extra'));
