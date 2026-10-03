// When `arguments` is only read for its length, native code can use the frame argc directly.
// Mutation, aliasing, indexed reads and an arrow closure all require the ordinary arguments object.
function count(a, b) { return arguments.length }
function changed(a) {
  arguments.length = 7
  return arguments.length
}
function aliased(a) {
  const args = arguments
  args.length = 9
  return arguments.length
}
function captured(a) {
  const setLength = () => {
    arguments.length = 11
  }
  setLength()
  return arguments.length
}
function indexed(a) { return arguments[0] }
function mergedAndChanged(a, flag) {
  const args = flag ? arguments : arguments
  args.length = 17
  return arguments.length
}
function escapesAfterRead(a) {
  const length = arguments.length
  return [length, arguments]
}
function lengthThenReturn(a) {
  const length = arguments.length
  return length === 2 ? arguments : length
}

const escaped = escapesAfterRead(1, 2)
escaped[1].length = 19
const returned = lengthThenReturn(1, 2)
console.log(
  count(1, 2, 3),
  changed(1),
  aliased(1),
  captured(1),
  indexed(23),
  mergedAndChanged(1, true),
  escaped[0],
  escaped[1].length,
  returned.length
)
