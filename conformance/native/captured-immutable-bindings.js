// Immutable binding writes fail at PutValue, preserving RHS and numeric coercion effects.
let effects = 0;
function capture() {
  const fixed = { valueOf() { effects++; return 7; } };
  return {
    assign() { fixed = (++effects, 8); },
    update() { fixed++; },
    skip() { return fixed ||= (++effects, 9); },
    shadow(fixed) { return ++fixed; },
  };
}
const read = capture();
for (const call of [() => read.assign(), () => read.update()]) {
  let caught = false;
  try { call(); } catch (error) { caught = error instanceof TypeError; }
  if (!caught) throw new Error('captured immutable binding accepted a write');
}
if (effects !== 2 || !read.skip() || effects !== 2 || read.shadow(3) !== 4)
  throw new Error('captured immutable binding ordering or shadowing changed');
function beforeInitialization() {
  let caught = false;
  try { later = (++effects, 1); } catch (error) {
    if (!(error instanceof ReferenceError)) throw error;
    caught = true;
  }
  if (!caught) throw new Error('TDZ assignment accepted a write');
  const later = 2;
}
beforeInitialization();
if (effects !== 3) throw new Error('TDZ assignment skipped RHS effects');
if (false) { const dead = 1; dead = 2; }
console.log('captured-immutable-bindings:ok', effects);
