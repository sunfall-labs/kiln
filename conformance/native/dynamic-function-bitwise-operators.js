const installMask = new Function("Ctor", "name", "pos", `
  var mask = 1 << pos;
  Object.defineProperty(Ctor.prototype, name, {
    get: function() { return !!(this._bits & mask); },
    set: function(val) {
      if (val) this._bits |= mask;
      else this._bits &= ~mask;
    },
  });
`);
function Flags() { this._bits = 0; }
installMask(Flags, "active", 3);
const flags = new Flags();
flags.active = true;
const afterSet = [flags.active, flags._bits];
flags.active = false;
console.log(JSON.stringify({
  afterSet,
  afterClear: [flags.active, flags._bits],
  shifts: [1 << 31, -1 >> 1, -1 >>> 1],
  precedence: [1 + 2 << 2, 1 | 2 & 4, 1 === 1 & 0],
  regexAfterCompound: new Function("var x = 1; x &= /a/.test('a'); return x;")(),
  regexAfterShiftCompound: new Function("var x = 1; x <<= /a/.test('a') ? 2 : 0; return x;")(),
  regexAfterBitNot: new Function("return ~/a/.test('a');")(),
}));
