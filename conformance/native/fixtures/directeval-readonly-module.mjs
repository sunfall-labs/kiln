export function makeReadonly(source) {
  const captured = 'immutable';
  return eval(source);
}
export function makeNamed(source) {
  return eval(source);
}
