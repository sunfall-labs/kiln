class Instant extends Date {
  #flag = true;
  constructor(value) { super(value); }
  flag() { return this.#flag; }
}
const date = new Instant('1979-05-27T07:32:00Z');
console.log(Object.prototype.toString.call(date), date instanceof Date, date instanceof Instant);
console.log(date.getTime(), Date.prototype.getTime.call(date), date.flag());
console.log(date.toISOString());
