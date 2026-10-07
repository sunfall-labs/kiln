console.log('radix', BigInt('0xff').toString(), BigInt('0o77').toString(), BigInt('0b101').toString(), BigInt('  ').toString());
console.log('signed', BigInt(' -123 ').toString());
try { BigInt('-0x1'); } catch (error) { console.log(error.name); }
