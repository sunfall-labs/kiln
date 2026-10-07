// StringToBigInt trims the ECMAScript WhiteSpace and LineTerminator set by UTF-16 code unit.
console.log(BigInt("\u00a0\ufeff 0xff\u2028").toString());
console.log(BigInt("\u3000\u2029").toString());
console.log(BigInt("\u2007-123\u205f").toString());
try {
  BigInt("0x\u00a0");
} catch (error) {
  console.log(error.name);
}
