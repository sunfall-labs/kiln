function errorName(fn) {
  try {
    fn();
    return "none";
  } catch (error) {
    return error.name;
  }
}

console.log("roundtrip:", Uint8Array.fromHex("00aBcDff").toHex());
console.log("empty:", Uint8Array.fromHex("").length);
console.log("odd:", errorName(() => Uint8Array.fromHex("abc")));
console.log("invalid:", errorName(() => Uint8Array.fromHex("0g")));
console.log("type:", errorName(() => Uint8Array.fromHex(12)));
