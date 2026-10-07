module.exports = function checkKeptTarget(ref) {
  const allocations = [];
  for (let i = 0; i < 32; i++) allocations.push({ index: i });
  return ref.deref() !== undefined;
};
