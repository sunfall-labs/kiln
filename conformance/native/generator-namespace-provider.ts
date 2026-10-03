export function *step(value: number) {
  yield value + 1
  return value + 2
}
