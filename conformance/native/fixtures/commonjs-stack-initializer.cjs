function getStack() {
  const target = {};
  Error.captureStackTrace(target);
  return target.stack.slice(1);
}
function initialize() {
  const sites = getStack();
  // depd asks for its module-level caller after removing getStack itself.
  return typeof sites[1].getFileName();
}
module.exports = initialize();
