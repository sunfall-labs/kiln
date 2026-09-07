// An async function's parameter binding runs inside its first step, not at the call
// (`binds_at_call` excludes it, so an instantiation failure rejects rather than throws), and that
// step used to be entered with no argument list at all: `...rest` came back empty and `arguments`
// came back length 0, while a plain function, a generator and an async generator -- all of which
// bind at the call -- were right. Nothing threw, so a program read the wrong value three frames
// later, which is the expensive answer. The first step now carries the call's arguments.
//
// Every line is awaited in order and printed from one place, because the two engines interleave
// an async generator's first `next` against plain async functions differently and this probe is
// not about that.
const target = { m(x: number) { return x + 1; } };

async function af(...a: any[]) { return a.length + ":" + a.join(","); }
const arrowFn = async (...a: any[]) => a.length + ":" + a.join(",");
const holder = { async method(...a: any[]) { return a.length + ":" + a.join(","); } };
async function* agen(...a: any[]) { yield a.length + ":" + a.join(","); }
async function mixed(x: number, ...a: any[]) { return x + "|" + a.length + ":" + a.join(","); }
async function counted(_a?: number) { return "argc=" + arguments.length + " [" + Array.prototype.join.call(arguments, ",") + "]"; }
async function applied(...a: any[]) { return target.m.apply(target, a as [number]); }

const proxied: any = new Proxy(holder, { get(t, k, r) { return Reflect.get(t, k, r); } });

async function main() {
  const lines: string[] = [];
  lines.push("direct      " + await af(1, 2, 3));
  lines.push("apply       " + await af.apply(null, [1, 2, 3]));
  lines.push("call        " + await af.call(null, 1, 2, 3));
  lines.push("reflect     " + await Reflect.apply(af, null, [1, 2, 3]));
  lines.push("arrow       " + await arrowFn(1, 2, 3));
  lines.push("arrow-apply " + await arrowFn.apply(null, [4, 5]));
  lines.push("method      " + await holder.method(1, 2, 3));
  lines.push("method-call " + await holder.method.call(holder, 9));
  lines.push("proxy-get   " + await proxied.method(1, 2, 3));
  lines.push("agen        " + (await agen(1, 2, 3).next()).value);
  lines.push("agen-empty  " + (await agen().next()).value);
  lines.push("mixed       " + await mixed(1, 2, 3));
  lines.push("mixed-one   " + await mixed(1));
  lines.push("empty       " + await af());
  lines.push("undef-arg   " + await af(undefined, 1));
  lines.push("objects     " + await af([1, 2], { a: 1 }, "s", null).then((s: string) => s.length > 0 ? s : "?"));
  lines.push("nested-len  " + await af([1, 2], { a: 1 }).then((s: string) => s.split(":")[0]));
  lines.push("arguments   " + await counted(7, 8 as any));
  lines.push("args-empty  " + await counted());
  lines.push("forwarded   " + await applied(41));
  lines.push("forward-two " + await applied(1, 99));
  // The rest array is a fresh ordinary Array each call, and not shared between them.
  const first = await af(1);
  const second = await af(1, 2);
  lines.push("distinct    " + first + " / " + second);
  async function shape(...a: any[]) { return [Array.isArray(a), a.constructor === Array, a.length].join(","); }
  lines.push("shape       " + await shape(1, 2));
  lines.push("shape-empty " + await shape());
  for (const line of lines) console.log(line);
}

main().then(() => console.log("done"));
