// The three generator methods are separate built-ins, and captured methods retain their request
// kind when called later with the generator as receiver.
async function main() {
  async function* sequence() {
    yield "first";
    return "body-return";
  }
  const iterator = sequence();
  const prototype = Object.getPrototypeOf(iterator);
  const methods = [prototype.next, prototype.return, prototype.throw];
  if (new Set(methods).size !== 3) throw new Error("async generator method identities collapsed");

  const first = await iterator.next();
  const capturedReturn = iterator.return;
  const returned = await Reflect.apply(capturedReturn, iterator, ["explicit-return"]);
  console.log(`${first.value}/${first.done}:${returned.value}/${returned.done}`);
}

main().catch((error) => { console.error(error); process.exit(1); });
