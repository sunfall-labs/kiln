console.log(+"");
console.log(+" \t\v\f\n\r");
console.log(+"\u00a0\u1680\u2000\u2028\u2029\u202f\u205f\u3000\ufeff1\ufeff");
console.log(+"\u00851\u0085");

console.log(+"1", +"1.", +".5", +"01.25e+2");
console.log(+"Infinity", +"+Infinity", +"-Infinity");
console.log(+"inf", +"INFINITY", +"NaN");
console.log(1 / +"-0", 1 / +"-1e-9999");
console.log(+"1e309", +"-1e309", +"5e-324", +"1e-324");

console.log(+"0x10", +"0XfF", +"0b11", +"0o10");
console.log(+"0x20000000000001", +"0x20000000000003");
console.log(+"0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");

console.log(+"0x", +"0b", +"0o", +"+0x10", +"-0x10");
console.log(+"1e", +"1e+", +"1_000", +"1.2x", +"Infinityx", +"0x1p2");
