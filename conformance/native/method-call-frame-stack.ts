// A direct method call pushes its frame inline; the method must still be in the stack of an
// error thrown inside it, and a sloppy function called as a method keeps its `this` substitution.
class Box {
  v = 1;
  boom(): number {
    if (this.v > 0) throw new Error("boom");
    return 0;
  }
  outer(): number { return this.boom(); }
}
const b = new Box();
function names(e: any): string {
  return String(e.stack).split("\n").slice(1).map((l: string) => (/at (?:\w+\.)?(\w+)/.exec(l) || [])[1]).filter((n: any) => n === "boom" || n === "outer").join(",");
}
for (let i = 0; i < 3; i++) {
  try { b.outer(); } catch (e) { console.log("stack", names(e)); }
}
// A sloppy function reached as a method of an object whose receiver becomes undefined via call.
function sloppy(this: any) { return this === undefined ? "undef" : typeof this; }
const holder: any = { sloppy };
console.log("sloppy", holder.sloppy(), sloppy.call(undefined), sloppy.call(null));
