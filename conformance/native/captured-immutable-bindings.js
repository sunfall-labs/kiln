// Immutable binding writes fail at PutValue, preserving RHS and numeric coercion effects.
let effects = 0;
function capture() {
  const fixed = { valueOf() { effects++; return 7; } };
  return {
    assign() { fixed = (++effects, 8); },
    update() { fixed++; },
    skip() { return fixed ||= (++effects, 9); },
    remove() { return delete fixed; },
    shadow(fixed) { return ++fixed; },
  };
}
const read = capture();
for (const call of [() => read.assign(), () => read.update()]) {
  let caught = false;
  try { call(); } catch (error) { caught = error instanceof TypeError; }
  if (!caught) throw new Error('captured immutable binding accepted a write');
}
if (effects !== 2 || !read.skip() || effects !== 2 || read.remove() !== false || read.shadow(3) !== 4)
  throw new Error('captured immutable binding ordering or shadowing changed');
function beforeInitialization() {
  const remove = () => delete later;
  if (remove() !== false) throw new Error('delete read a TDZ binding or treated it as absent');
  try { later = (++effects, 1); } catch (error) {
    if (!(error instanceof ReferenceError)) throw error;
  }
  const later = 2;
}
beforeInitialization();
if (effects !== 3) throw new Error('TDZ assignment skipped RHS effects');
if (false) { const dead = 1; dead = 2; }
console.log('captured-immutable-bindings:ok', effects);
